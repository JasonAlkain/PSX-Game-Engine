//! `copperframe_player`: plays projects without gameplay code. Behaviours other
//! than the engine's `psx.*` kinds make a scene refuse to load with a clear
//! message; such projects need a game crate (`copperframe new` creates one).

use copperframe_game::{Game, Registry};

struct StockGame;

impl Game for StockGame {
    fn title(&self) -> String {
        "Copperframe Engine player".into()
    }

    fn register(&self, _registry: &mut Registry) {}
}

fn main() -> std::process::ExitCode {
    copperframe_game::main(|| Box::new(StockGame), ".")
}
