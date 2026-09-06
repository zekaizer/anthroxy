use super::style::table;
use super::{Cli, Style};
use crate::routing::{Registry, Route};

pub fn run(cli: &Cli, style: &Style) -> anyhow::Result<()> {
    let config = cli.load_config()?;
    let registry = Registry::from_config(&config)?;
    print!("{}", table(&table_rows(&registry, style, None)));
    println!();
    println!("{}", default_route_line(&registry, style));
    Ok(())
}

/// A header row plus one row per route: `mark` (when given), id, backend,
/// upstream model, picker label, aliases.
pub fn table_rows(
    registry: &Registry,
    style: &Style,
    mark: Option<&dyn Fn(&Route) -> String>,
) -> Vec<Vec<String>> {
    let mut header: Vec<String> = Vec::new();
    if mark.is_some() {
        header.push(String::new());
    }
    header.extend(
        ["id", "backend", "upstream model", "picker label", "aliases"]
            .iter()
            .map(|h| style.dim(h)),
    );
    let mut rows = vec![header];
    for route in registry.routes() {
        let mut row = Vec::new();
        if let Some(mark) = mark {
            row.push(mark(route));
        }
        row.extend([
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
        rows.push(row);
    }
    rows
}

/// Where requests naming an unlisted model go.
pub fn default_route_line(registry: &Registry, style: &Style) -> String {
    match registry.default_route() {
        Some(route) => format!("unknown model ids → {}", style.bold(&route.id)),
        None => "unknown model ids → rejected with 404".to_owned(),
    }
}
