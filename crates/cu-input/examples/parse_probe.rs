// Ejemplo de solo lectura: no inyecta input. Solo reporta qué tools están nativos.
fn main() {
    for name in [
        "computer_hotkey",
        "computer_key",
        "computer_move",
        "computer_copy",
        "computer_paste",
        "computer_bulk_input",
        "computer_drag_scroll",
        "computer_click",
    ] {
        println!("{} => {}", name, cu_input::handles(name));
    }
}