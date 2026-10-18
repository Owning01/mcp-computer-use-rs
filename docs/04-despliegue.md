# 04 - Despliegue / cutover

Local (esta maquina). El cutover es por config de OpenCode y es reversible:

1. Editar C:\Users\perca\.config\opencode\opencode.json -> mcp.<server>.command al binario Rust.
2. Reiniciar la sesion de OpenCode.
3. Rollback: restaurar la entrada anterior. El origen queda intacto hasta F8.

F8: apagar el puente por componente, correr e2e completa, borrar/archivar el origen y actualizar su PROJECT_MAP/PROJECT_MEMORY.