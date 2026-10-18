# 01 - Setup

Requisitos: Rust 1.98+ (rustup), Windows 11. Para correr el origen durante la migracion: Node 22+ y su config actual.

    cargo build --release
    # binario: target/release/computer-use2-rs.exe

Config MCP en OpenCode (se aplica recien en el cutover; ver migration/CHARTER.md):

    "mcp": { "computer-use2": { "type": "local", "command": ["<ruta>/target/release/computer-use2-rs.exe"], "enabled": true } }
