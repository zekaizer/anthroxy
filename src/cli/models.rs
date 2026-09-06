use super::style::table;
use super::{Cli, Style};
use crate::routing::Registry;

pub fn run(cli: &Cli, style: &Style) -> anyhow::Result<()> {
    let config = cli.load_config()?;
    let registry = Registry::from_config(&config)?;
    print!("{}", render(&registry, style));
    Ok(())
}

pub fn render(registry: &Registry, style: &Style) -> String {
    let mut rows = vec![
        ["id", "backend", "upstream model", "picker label", "aliases"]
            .iter()
            .map(|h| style.dim(h))
            .collect::<Vec<_>>(),
    ];
    for route in registry.routes() {
        rows.push(vec![
            style.bold(&route.id),
            route.backend.name.clone(),
            route.upstream_model.clone(),
            route.display_name.clone(),
            if route.aliases.is_empty() {
                "-".to_owned()
            } else {
                route.aliases.join(", ")
            },
        ]);
    }
    let mut out = table(&rows);
    out.push('\n');
    match registry.default_route() {
        Some(route) => out.push_str(&format!(
            "Unknown model ids are routed to {}.\n",
            style.bold(&route.id)
        )),
        None => out.push_str("Unknown model ids are rejected with 404.\n"),
    }
    out
}
