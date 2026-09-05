use std::collections::HashMap;

use crate::config::Config;

/// One exposed model and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Identifier exposed to Claude Code.
    pub id: String,
    pub backend: String,
    /// Value written into the upstream request's `model` field.
    pub upstream_model: String,
    pub display_name: String,
    pub aliases: Vec<String>,
}

/// Why a request matched a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    /// The request named the route's `id`.
    Exact,
    /// The request named one of the route's aliases.
    Alias,
    /// The model was unknown and `routing.default_model` applied.
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution<'a> {
    pub route: &'a Route,
    pub matched: Match,
}

/// Immutable model table built once from configuration.
#[derive(Debug, Clone)]
pub struct Registry {
    routes: Vec<Route>,
    by_name: HashMap<String, usize>,
    default: Option<usize>,
}

impl Registry {
    /// Assumes `config` passed validation: backends exist, names are unique.
    pub fn from_config(config: &Config) -> Registry {
        let routes: Vec<Route> = config
            .models
            .iter()
            .map(|m| Route {
                id: m.id.clone(),
                backend: m.backend.clone(),
                upstream_model: m.upstream_model.clone().unwrap_or_else(|| m.id.clone()),
                display_name: m.display_name.clone().unwrap_or_else(|| m.id.clone()),
                aliases: m.aliases.clone(),
            })
            .collect();
        let mut by_name = HashMap::new();
        for (index, route) in routes.iter().enumerate() {
            by_name.insert(route.id.clone(), index);
            for alias in &route.aliases {
                by_name.insert(alias.clone(), index);
            }
        }
        let default = config
            .routing
            .default_model
            .as_deref()
            .and_then(|name| by_name.get(name).copied());
        Registry {
            routes,
            by_name,
            default,
        }
    }

    /// Routes in configuration order, i.e. picker order.
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    pub fn resolve(&self, model: &str) -> Option<Resolution<'_>> {
        if let Some(&index) = self.by_name.get(model) {
            let route = &self.routes[index];
            let matched = if route.id == model {
                Match::Exact
            } else {
                Match::Alias
            };
            return Some(Resolution { route, matched });
        }
        self.default.map(|index| Resolution {
            route: &self.routes[index],
            matched: Match::Default,
        })
    }

    /// Ids and aliases a client may name, in configuration order.
    pub fn known_names(&self) -> Vec<&str> {
        self.routes
            .iter()
            .flat_map(|r| {
                std::iter::once(r.id.as_str()).chain(r.aliases.iter().map(String::as_str))
            })
            .collect()
    }

    pub fn default_route(&self) -> Option<&Route> {
        self.default.map(|i| &self.routes[i])
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}
