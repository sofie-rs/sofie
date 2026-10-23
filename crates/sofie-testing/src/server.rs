use std::sync::Arc;
use std::time::Duration;

use http::Method;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use sofie_auth::SessionClaims;
use sofie_core::{Error, Result, Timestamp, Uuid};
use sofie_http::{App, CookieNames, SigningKeys};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::request::{HttpClient, TestRequest, TestSession};

const STOP_TIMEOUT: Duration = Duration::from_secs(5);

pub struct TestServer {
    base: String,
    keys: Arc<SigningKeys>,
    cookies: CookieNames,
    client: HttpClient,
    stop: Option<oneshot::Sender<()>>,
    served: Option<JoinHandle<Result<()>>>,
}

impl TestServer {
    pub async fn start(app: App) -> Self {
        let mut cookies = None;
        let mut key = [0; 32];
        sofie_core::random::fill(&mut key).expect("generating a session key");
        let keys = || SigningKeys::new(&[key]).expect("a 32-byte session key");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding a local port");
        let address = listener.local_addr().expect("reading the bound address");
        let (stop, stopped) = oneshot::channel::<()>();
        let app = app.serving(|serving| {
            cookies = Some(CookieNames::of_serving(&serving));
            serving.signing_keys(keys())
        });
        let cookies = cookies
            .expect("`App::serving` configures at once")
            .expect("an app name that names a cookie");
        let served = tokio::spawn(app.serve(listener, async {
            let _ = stopped.await;
        }));
        let served = started(served).await;
        Self {
            base: format!("http://{address}"),
            keys: Arc::new(keys()),
            cookies,
            client: Client::builder(TokioExecutor::new()).build_http(),
            stop: Some(stop),
            served: Some(served),
        }
    }

    #[must_use]
    pub fn signing_keys(&self) -> Arc<SigningKeys> {
        Arc::clone(&self.keys)
    }

    #[must_use]
    pub fn origin(&self) -> &str {
        &self.base
    }

    pub fn get(&self, path: &str) -> TestRequest {
        self.request(Method::GET, path)
    }

    pub fn post(&self, path: &str) -> TestRequest {
        self.request(Method::POST, path)
    }

    pub fn request(&self, method: Method, path: &str) -> TestRequest {
        TestRequest::new(
            self.client.clone(),
            method,
            format!("{}{path}", self.base),
            self.cookies.clone(),
        )
    }

    pub async fn stop(mut self) -> Result<()> {
        self.signal_stop();
        let served = self.served.take().expect("a server is stopped once");
        tokio::time::timeout(STOP_TIMEOUT, served)
            .await
            .map_err(|error| Error::internal("waiting for the server to stop", error))?
            .map_err(|error| Error::internal("joining the server task", error))?
    }

    pub(crate) fn seal(&self, area: &str, principal: Uuid, now: Timestamp) -> TestSession {
        let session = Uuid::new_v4().expect("choosing a test session's id");
        let claims = SessionClaims::new(session, area, principal, now);
        let sealed = claims.seal(&self.keys).expect("sealing a test session");
        TestSession::new(format!("{}={sealed}", self.cookies.session()))
    }

    pub(crate) fn open(&self, session: &TestSession, now: Timestamp) -> Option<SessionClaims> {
        let sealed = session
            .cookie()
            .strip_prefix(self.cookies.session())?
            .strip_prefix('=')?;
        SessionClaims::open(&self.keys, sealed, now).ok()
    }

    fn signal_stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// The app's checks run before it first waits on the listener, so one yield lets a failed boot
/// end the task, and the test fails with its reason rather than a refused connection later.
async fn started(mut served: JoinHandle<Result<()>>) -> JoinHandle<Result<()>> {
    tokio::task::yield_now().await;
    if !served.is_finished() {
        return served;
    }
    let reason = match (&mut served).await {
        Ok(Ok(())) => "it stopped at once".to_owned(),
        Ok(Err(error)) => error.to_string(),
        Err(error) => error.to_string(),
    };
    panic!("the test app did not start: {reason}");
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.signal_stop();
    }
}
