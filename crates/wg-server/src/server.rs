//! [`Server`]: the state every request shares (`create_app`), and [`serve`].

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::spec::pairing::{Pairing, pairing_code};
use crate::spec::routes::Entry;
use crate::spec::security::{Lan, Policy};
use crate::spec::{Target, openapi_body, route_table};
use axum::Router;
use wg_iface::http::request::{Declared, operation, params};
use wg_runtime::{App, Bus, draw};
use wg_types::WgError;

use crate::route::ApiRoute;
use crate::ui::Ui;

/// Serving the UI to devices on the local network (`LanAccess.build`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanOptions {
    /// The server's address on the LAN.
    pub address: String,
    /// `server.lan_networks`; empty for the private ranges.
    pub networks: Vec<String>,
    /// `socket.gethostname()`.
    pub hostname: String,
    /// The pairing code; `None` draws a fresh one.
    pub pairing_code: Option<String>,
}

/// How to build the server (`create_app`'s arguments).
pub struct ServerOptions {
    /// The bound host, which the allowed hosts and origins name.
    pub host: String,
    pub port: u16,
    pub lan: Option<LanOptions>,
    pub ui: Ui,
    /// The MCP Streamable HTTP service to mount at `/mcp`, when there is one.
    pub mcp: Option<Router>,
    /// The API routes, in any order; they are registered in the Python order.
    pub routes: Vec<ApiRoute>,
}

/// The state of one HTTP server.
pub struct Server {
    /// The security policy: the token, the allowed hosts and origins, the LAN settings.
    pub policy: Policy,
    /// The process's signal bus, for event streams.
    pub bus: Bus,
    pub(crate) table: Vec<Entry<Target>>,
    pub(crate) routes: Vec<(ApiRoute, Declared)>,
    pub(crate) pairing: Option<Mutex<Pairing>>,
    pub(crate) ui: Ui,
    pub(crate) mcp: Option<Router>,
    pub(crate) openapi: Vec<u8>,
    app: Arc<Mutex<App>>,
}

/// Locks a mutex; a panic while it was held does not poison the server.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Server {
    /// `create_app(app, host=..., port=..., lan=...)`: reads the API token (creating it when
    /// missing), the `WORKGRAPH_ALLOWED_HOSTS` and `WORKGRAPH_ALLOWED_ORIGINS` overrides, and the
    /// parameter schemas of each route from the OpenAPI snapshot.
    pub fn new(options: ServerOptions, app: App) -> Result<Server, WgError> {
        let setup_error = |message: String| WgError::config(message);
        let token = app.api_token(true)?;
        let lan = options
            .lan
            .as_ref()
            .map(|lan| Lan::build(&lan.address, &lan.networks, &lan.hostname))
            .transpose()
            .map_err(setup_error)?;
        let code = match options.lan.as_ref().map(|lan| lan.pairing_code.clone()) {
            Some(Some(code)) => Some(code),
            Some(None) => Some(loop {
                // `_new_pairing_code()` from the OS random source.
                if let Some(code) = pairing_code(&draw()?.1) {
                    break code;
                }
            }),
            None => None,
        };
        let policy = Policy::new(
            &token,
            &options.host,
            options.port,
            lan,
            std::env::var("WORKGRAPH_ALLOWED_HOSTS").ok().as_deref(),
            std::env::var("WORKGRAPH_ALLOWED_ORIGINS").ok().as_deref(),
        );
        let openapi_doc = wg_wire::contract::openapi()?;
        let mut routes = options.routes;
        routes.sort_by_key(|route| {
            wg_iface::http::operations::registration_index(route.id).unwrap_or(usize::MAX)
        });
        let routes = routes
            .into_iter()
            .map(|route| {
                let op = operation(&openapi_doc, route.method, route.path).ok_or_else(|| {
                    setup_error(format!(
                        "route {} {} {} is not in the OpenAPI snapshot",
                        route.id, route.method, route.path
                    ))
                })?;
                let declared = Declared {
                    params: params(&openapi_doc, op).map_err(setup_error)?,
                    body: route.body.clone(),
                };
                Ok((route, declared))
            })
            .collect::<Result<Vec<_>, WgError>>()?;
        let keys: Vec<(&str, &str)> = routes
            .iter()
            .map(|(route, _)| (route.method, route.path))
            .collect();
        let table = route_table(&keys, options.mcp.is_some());
        Ok(Server {
            policy,
            table,
            pairing: code.map(|code| Mutex::new(Pairing::new(&code))),
            ui: options.ui,
            mcp: options.mcp,
            openapi: openapi_body()?,
            bus: app.runtime.effects.bus.clone(),
            app: Arc::new(Mutex::new(app)),
            routes,
        })
    }

    /// The pairing code `serve` prints, when LAN serving is on.
    pub fn pairing_code(&self) -> Option<String> {
        self.pairing.as_ref().map(|state| lock(state).code.clone())
    }

    /// Runs `work` on the application context on a blocking thread (section 4.11): the runtime
    /// is synchronous, and requests share the one context in turn.
    pub async fn blocking<T, F>(&self, work: F) -> Result<T, WgError>
    where
        T: Send + 'static,
        F: FnOnce(&mut App) -> Result<T, WgError> + Send + 'static,
    {
        let app = Arc::clone(&self.app);
        tokio::task::spawn_blocking(move || work(&mut lock(&app)))
            .await
            .map_err(|err| WgError::workgraph(format!("request task failed: {err}")))?
    }

    /// One pairing attempt under the lock: `apply` gets the current state (`None` without LAN
    /// serving) and returns the reply and the next state.
    pub(crate) fn with_pairing<R>(
        &self,
        apply: impl FnOnce(Option<&Pairing>) -> (R, Option<Pairing>),
    ) -> R {
        match &self.pairing {
            None => apply(None).0,
            Some(state) => {
                let mut guard = lock(state);
                let (reply, next) = apply(Some(&guard));
                if let Some(next) = next {
                    *guard = next;
                }
                reply
            }
        }
    }
}

/// Serves the application on each listener until one fails (the loopback listener, and the LAN
/// listener when there is one, share the server as uvicorn's sockets share the app).
pub async fn serve(
    server: Arc<Server>,
    listeners: Vec<tokio::net::TcpListener>,
) -> std::io::Result<()> {
    let tasks: Vec<_> = listeners
        .into_iter()
        .map(|listener| {
            let app = crate::dispatch::router(Arc::clone(&server))
                .into_make_service_with_connect_info::<std::net::SocketAddr>();
            tokio::spawn(async move { axum::serve(listener, app).await })
        })
        .collect();
    for task in tasks {
        task.await.map_err(std::io::Error::other)??;
    }
    Ok(())
}
