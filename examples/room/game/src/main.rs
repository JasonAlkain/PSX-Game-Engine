//! Game executable for the example project. Run from the repository with
//! `cargo run -p lever_room` (or `-- --help` for headless and editor options).

fn main() -> std::process::ExitCode {
    // The project is the folder above this crate during development; an
    // exported build uses the `data` folder next to the executable.
    psx_game::main(
        || Box::new(lever_room::LeverRoom),
        concat!(env!("CARGO_MANIFEST_DIR"), "/.."),
    )
}
