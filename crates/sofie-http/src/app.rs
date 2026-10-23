use std::collections::HashMap;
use std::sync::Arc;

use sofie_core::Result;
use sofie_db::Db;
use tokio::net::TcpListener;

#[cfg(feature = "engine")]
use crate::Screens;
use crate::admission::AreaPolicy;
use crate::routes::Route;
use crate::throttle::{Limiter, Rate};
use crate::{Area, AreaMembers, RouteName, Routes, Serving, server};

pub struct App {
    pub(crate) db: Db,
    #[cfg(feature = "engine")]
    pub(crate) screens: Screens,
    pub(crate) routes: Vec<Route>,
    #[cfg(feature = "islands")]
    pub(crate) islands: Vec<crate::islands::IslandScreen>,
    pub(crate) names: Vec<RouteName>,
    pub(crate) areas: HashMap<&'static str, AreaPolicy>,
    pub(crate) members: Option<Arc<dyn AreaMembers>>,
    #[cfg(feature = "gates")]
    pub(crate) gates: crate::Gates,
    #[cfg(feature = "steps")]
    pub(crate) steps: Vec<Box<dyn crate::PipelineStep>>,
    #[cfg(feature = "cors")]
    pub(crate) cors: HashMap<&'static str, crate::Cors>,
    pub(crate) serving: Serving,
}

impl App {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self {
            db,
            #[cfg(feature = "engine")]
            screens: Screens::default(),
            routes: Vec::new(),
            #[cfg(feature = "islands")]
            islands: Vec::new(),
            names: Vec::new(),
            areas: HashMap::new(),
            members: None,
            #[cfg(feature = "gates")]
            gates: crate::Gates::default(),
            #[cfg(feature = "steps")]
            steps: Vec::new(),
            #[cfg(feature = "cors")]
            cors: HashMap::new(),
            serving: Serving::default(),
        }
    }

    #[cfg(feature = "steps")]
    #[must_use]
    pub fn step<A: Area>(mut self, step: impl crate::PipelineStep) -> Self {
        self.areas
            .entry(A::NAME)
            .or_default()
            .steps
            .push(Box::new(step));
        self
    }

    /// Runs on every routed request before the area's steps, and on every answer, a 404 too.
    #[cfg(feature = "steps")]
    #[must_use]
    pub fn step_all(mut self, step: impl crate::PipelineStep) -> Self {
        self.steps.push(Box::new(step));
        self
    }

    /// Replaces the area's earlier policy.
    #[cfg(feature = "cors")]
    #[must_use]
    pub fn cors<A: Area>(mut self, cors: crate::Cors) -> Self {
        self.cors.insert(A::NAME, cors);
        self
    }

    /// Keeps the menus given so far, which live beside the gates.
    #[cfg(feature = "gates")]
    #[must_use]
    pub fn gates(mut self, gates: crate::Gates) -> Self {
        #[cfg(feature = "menu")]
        let menus = std::mem::take(&mut self.gates.menus);
        self.gates = gates;
        #[cfg(feature = "menu")]
        {
            self.gates.menus = menus;
        }
        self
    }

    /// Besides any route's own limit.
    #[must_use]
    pub fn throttle<A: Area>(mut self, rate: Rate) -> Self {
        self.areas.entry(A::NAME).or_default().throttle = Some(Limiter::new(rate));
        self
    }

    /// Where grants, bearer tokens and lock notices are read.
    #[must_use]
    pub fn membership(mut self, members: impl AreaMembers) -> Self {
        self.members = Some(Arc::new(members));
        self
    }

    /// Changes what was set before, so the process and a test server each add their own.
    #[must_use]
    pub fn serving(mut self, configure: impl FnOnce(Serving) -> Serving) -> Self {
        self.serving = configure(std::mem::take(&mut self.serving));
        self
    }

    #[cfg(feature = "engine")]
    #[must_use]
    pub fn screens(mut self, screens: Screens) -> Self {
        self.screens = screens;
        self
    }

    #[must_use]
    pub fn area<A: Area>(
        mut self,
        module: &'static str,
        define: impl FnOnce(&mut Routes<A>),
    ) -> Self {
        let mut routes = Routes::new(module);
        define(&mut routes);
        self.routes.extend(routes.added);
        #[cfg(feature = "islands")]
        self.islands.extend(routes.islands);
        self
    }

    /// Finishes open requests after `shutdown` completes.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()>,
    ) -> Result<()> {
        server::serve(self, listener, shutdown).await
    }
}
