#!/usr/bin/env python3
"""Persistent local OCR worker for the computer-use MCP server.

The protocol is one JSON request per line and one JSON response per line.
PaddleOCR is loaded once and kept warm between calls.
"""

import base64
import io
import json
import sys
from contextlib import redirect_stdout


_engines = {}


def _box(value):
    if isinstance(value, dict):
        if all(key in value for key in ("x", "y", "w", "h")):
            return {key: int(round(float(value[key]))) for key in ("x", "y", "w", "h")}
        for key in ("rec_boxes", "dt_polys", "text_box"):
            if key in value:
                return _box(value[key])
    if isinstance(value, (list, tuple)):
        points = []
        for point in value:
            if isinstance(point, (list, tuple)) and len(point) >= 2 and all(isinstance(n, (int, float)) for n in point[:2]):
                points.append((float(point[0]), float(point[1])))
            elif isinstance(point, (list, tuple)):
                nested = _box(point)
                if nested:
                    points.append((nested["x"], nested["y"]))
                    points.append((nested["x"] + nested["w"], nested["y"] + nested["h"]))
        if points:
            xs = [point[0] for point in points]
            ys = [point[1] for point in points]
            return {"x": int(round(min(xs))), "y": int(round(min(ys))), "w": int(round(max(xs) - min(xs))), "h": int(round(max(ys) - min(ys)))}
    return None


def _jsonable(value):
    if isinstance(value, str):
        try:
            return json.loads(value)
        except json.JSONDecodeError:
            return value
    if hasattr(value, "json"):
        candidate = value.json
        return _jsonable(candidate() if callable(candidate) else candidate)
    if isinstance(value, dict):
        return {str(key): _jsonable(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_jsonable(item) for item in value]
    if hasattr(value, "tolist"):
        return _jsonable(value.tolist())
    return value


def _make_engine(language, device):
    key = (language, device)
    if key in _engines:
        return _engines[key]

    try:
        from paddleocr import PaddleOCR
    except Exception as exc:
        raise RuntimeError(
            "PaddleOCR no está instalado. Instala PaddlePaddle GPU y paddleocr en el Python configurado; "
            "el worker no instala paquetes automáticamente."
        ) from exc

    selected_device = device
    if selected_device == "auto":
        selected_device = "gpu:0"

    try:
        # PaddleOCR 3.x. These switches avoid document analysis overhead for UI text.
        with redirect_stdout(sys.stderr):
            engine = PaddleOCR(
                lang=language,
                device=selected_device,
                use_doc_orientation_classify=False,
                use_doc_unwarping=False,
                use_textline_orientation=False,
            )
    except TypeError:
        # Compatibility with PaddleOCR 2.x installations.
        with redirect_stdout(sys.stderr):
            engine = PaddleOCR(lang=language, use_angle_cls=False, use_gpu=selected_device.startswith("gpu"))
    _engines[key] = engine
    return engine


def _legacy_items(raw, minimum):
    items = []
    if isinstance(raw, list) and len(raw) == 1 and isinstance(raw[0], list):
        raw = raw[0]
    for line in raw or []:
        if not isinstance(line, (list, tuple)) or len(line) < 2:
            continue
        box = _box(line[0])
        pair = line[1]
        if not box or not isinstance(pair, (list, tuple)) or len(pair) < 2:
            continue
        text, score = str(pair[0]), float(pair[1])
        if score >= minimum and text.strip():
            items.append({"text": text, "confidence": score, **box})
    return items


def _result_items(result, minimum):
    data = _jsonable(result)
    if isinstance(data, dict):
        texts = data.get("rec_texts") or data.get("texts") or []
        scores = data.get("rec_scores") or data.get("scores") or []
        boxes = data.get("rec_boxes") or data.get("dt_polys") or data.get("boxes") or []
        items = []
        for index, text in enumerate(texts):
            score = float(scores[index]) if index < len(scores) else 1.0
            box = _box(boxes[index]) if index < len(boxes) else None
            if score >= minimum and str(text).strip() and box:
                items.append({"text": str(text), "confidence": score, **box})
        return items
    return _legacy_items(data, minimum)


def recognize(request):
    language = str(request.get("language") or "en")
    device = str(request.get("device") or "auto")
    minimum = float(request.get("min_confidence", 0.35))
    maximum = max(1, min(int(request.get("max_results", 200)), 1000))
    query = str(request.get("query") or "").strip().lower()
    group_lines = bool(request.get("group_lines"))
    image = base64.b64decode(request.get("image", ""))
    if not image:
        raise RuntimeError("empty_image")

    try:
        import cv2
        import numpy as np
        decoded = cv2.imdecode(np.frombuffer(image, dtype=np.uint8), cv2.IMREAD_COLOR)
    except Exception as exc:
        raise RuntimeError("OCR requiere numpy y opencv-python") from exc
    if decoded is None:
        raise RuntimeError("invalid_image")

    engine = _make_engine(language, device)
    with redirect_stdout(sys.stderr):
        if hasattr(engine, "predict"):
            raw_results = engine.predict(decoded)
            items = []
            for result in raw_results:
                items.extend(_result_items(result, minimum))
        else:
            items = _legacy_items(engine.ocr(decoded, cls=False), minimum)

    # If query was specified, filter directly for massive token savings
    if query:
        items = [it for it in items if query in it.get("text", "").lower()]

    # If line grouping requested, cluster words by vertical proximity
    if group_lines and items:
        # Sort by y then x
        items.sort(key=lambda it: (it.get("y", 0), it.get("x", 0)))
        grouped = []
        current_line = [items[0]]
        for it in items[1:]:
            last = current_line[-1]
            # If vertical center is within 60% of height of previous item, same line
            if abs(it.get("y", 0) - last.get("y", 0)) < max(12, int(last.get("h", 16) * 0.6)):
                current_line.append(it)
            else:
                text = " ".join(x.get("text", "") for x in current_line)
                min_x = min(x.get("x", 0) for x in current_line)
                min_y = min(x.get("y", 0) for x in current_line)
                max_r = max(x.get("x", 0) + x.get("w", 0) for x in current_line)
                max_b = max(x.get("y", 0) + x.get("h", 0) for x in current_line)
                avg_conf = sum(x.get("confidence", 0.9) for x in current_line) / len(current_line)
                grouped.append({"text": text, "x": min_x, "y": min_y, "w": max_r - min_x, "h": max_b - min_y, "confidence": round(avg_conf, 2)})
                current_line = [it]
        if current_line:
            text = " ".join(x.get("text", "") for x in current_line)
            min_x = min(x.get("x", 0) for x in current_line)
            min_y = min(x.get("y", 0) for x in current_line)
            max_r = max(x.get("x", 0) + x.get("w", 0) for x in current_line)
            max_b = max(x.get("y", 0) + x.get("h", 0) for x in current_line)
            avg_conf = sum(x.get("confidence", 0.9) for x in current_line) / len(current_line)
            grouped.append({"text": text, "x": min_x, "y": min_y, "w": max_r - min_x, "h": max_b - min_y, "confidence": round(avg_conf, 2)})
        items = grouped

    return {"backend": "paddleocr", "device": device, "items": items[:maximum]}


def main():
    for line in sys.stdin:
        try:
            request = json.loads(line)
            response = {"id": request.get("id"), **recognize(request)}
        except Exception as exc:
            response = {"id": request.get("id") if "request" in locals() else None, "error": str(exc)}
        sys.stdout.write(json.dumps(response, ensure_ascii=False, separators=(",", ":")) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
