# 02 - Arquitectura (destino)

Estado: F0/F1 (charter + inventario). Diseno definitivo se congela en F3 (rulebook); workspace Rust se crea en F4.

    OpenCode (MCP client)
        |  stdio JSON-RPC 2.0 (rmcp)
        v
    [computer-use2-rs]
        |-- router MCP: 68 tools, perfil agent/full, tasks extension
        |-- core portable: locks, incidents, macros, errores, batch
        |-- nativo (windows-rs): input/UIA/captura/captura WGC/ventanas/tray/toast/audio/D3D11
        |-- CDP client (websocket): map_ui, upload_file tier 1

    Reemplaza 4 capas del origen: Node + powershell.exe daemon + helpers C# (exes) + ocr_worker.py.
    El puente F4 proxea por stdio las tools aun no portadas al server Node viejo.

Puente F4: ver migration/CHARTER.md seccion 2. Baseline a medir antes del primer componente.
