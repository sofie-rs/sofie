use crate::{Area, Routes};

pub trait Groups {
    /// A record below `path` must hold the ids its path parameters name, or it is not found.
    fn group(&mut self, path: &str, define: impl FnOnce(&mut Self));
}

impl<A: Area> Groups for Routes<A> {
    fn group(&mut self, path: &str, define: impl FnOnce(&mut Self)) {
        let mut inner = Self::new(self.module);
        define(&mut inner);
        for mut route in inner.added {
            route.path.insert_str(A::PREFIX.len(), path);
            self.added.push(route);
        }
    }
}
