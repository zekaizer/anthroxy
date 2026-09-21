use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::config::{BackendKind, Config};
use crate::openai::SystemPlacement;
use crate::upstream::{Backend, BackendBuildError};

/// One exposed model and where it goes.
#[derive(Debug, Clone)]
pub struct Route {
    /// Identifier exposed to Claude Code.
    pub id: String,
    pub backend: Arc<Backend>,
    /// Value written into the upstream request's `model` field.
    pub upstream_model: String,
    pub display_name: String,
    pub aliases: Vec<String>,
    /// Where a mid-conversation system message goes on an `openai` backend:
    /// the model's setting, else the backend's.
    pub mid_conversation_system: SystemPlacement,
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

#[derive(Debug, Clone, Copy)]
pub struct Resolution<'a> {
    pub route: &'a Route,
    pub matched: Match,
}

/// The backends and the model table pointing at them, built once from
/// configuration and immutable afterwards.
#[derive(Debug)]
pub struct Registry {
    backends: BTreeMap<String, Arc<Backend>>,
    routes: Vec<Route>,
    by_name: HashMap<String, usize>,
    default: Option<usize>,
}

impl Registry {
    /// Assumes `config` passed validation: every model names a configured
    /// backend and ids are unique. Fails when a credential source cannot be
    /// set up.
    pub fn from_config(config: &Config) -> Result<Registry, BackendBuildError> {
        let mut backends = BTreeMap::new();
        for (name, cfg) in &config.backends {
            backends.insert(name.clone(), Arc::new(Backend::from_config(name, cfg)?));
        }
        let routes: Vec<Route> = config
            .models
            .iter()
            .map(|m| Route {
                id: m.id.clone(),
                backend: Arc::clone(&backends[&m.backend]),
                upstream_model: m.upstream_model.clone().unwrap_or_else(|| m.id.clone()),
                display_name: m.display_name.clone().unwrap_or_else(|| m.id.clone()),
                aliases: m.aliases.clone(),
                mid_conversation_system: m
                    .mid_conversation_system
                    .unwrap_or(backends[&m.backend].mid_conversation_system),
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
        Ok(Registry {
            backends,
            routes,
            by_name,
            default,
        })
    }

    /// Every configured backend, by name.
    pub fn backends(&self) -> impl ExactSizeIterator<Item = &Arc<Backend>> {
        self.backends.values()
    }

    /// Routes in configuration order, i.e. picker order.
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    /// Route named by id or alias; the default never applies here.
    pub fn lookup(&self, name: &str) -> Option<Resolution<'_>> {
        let &index = self.by_name.get(name)?;
        let route = &self.routes[index];
        let matched = if route.id == name {
            Match::Exact
        } else {
            Match::Alias
        };
        Some(Resolution { route, matched })
    }

    /// [`lookup`](Self::lookup), else the default route when one is configured.
    pub fn resolve(&self, name: &str) -> Option<Resolution<'_>> {
        self.lookup(name).or_else(|| {
            self.default.map(|index| Resolution {
                route: &self.routes[index],
                matched: Match::Default,
            })
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

    /// The unique `kind = "passthrough"` backend, if configured.
    pub fn passthrough(&self) -> Option<Arc<Backend>> {
        self.backends
            .values()
            .find(|backend| backend.kind == BackendKind::Passthrough)
            .cloned()
    }

    /// Backends whose model identity is fetched live.
    pub fn live_backends(&self) -> Vec<Arc<Backend>> {
        self.backends
            .values()
            .filter(|backend| backend.live_models)
            .cloned()
            .collect()
    }
}
