//! List the slash commands and skills Claude Code offers in a folder.
//!   cargo run -q -p promptly-core --example slash -- [dir] [filter]
fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let q = std::env::args().nth(2).unwrap_or_default();
    let items = promptly_core::commands::discover(&dir, &promptly_core::paths::claude_dir());
    let shown = promptly_core::commands::filter(&items, &q);
    println!("{} items ({} shown)", items.len(), shown.len());
    for i in shown.iter().take(12) {
        println!(
            "/{:<40} {:<8} {}",
            i.name,
            i.source.label(),
            i.description.chars().take(60).collect::<String>()
        );
    }
}
