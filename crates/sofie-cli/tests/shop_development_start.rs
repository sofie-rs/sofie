#![cfg(feature = "slow-tests")]

use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Built alone with `-p shop`, as `sofie dev` builds it, so no other member's features (a
/// dev-dependency's `i18n`) reach the screens it parses from their sources at start.
#[test]
fn the_shop_built_for_development_with_only_its_own_features_writes_its_sign_in_texts() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = shared_target();
    build_shop(&repository, &target);
    let log_path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shop-development-start.log");
    let address = free_address();
    let mut shop = ServedShop(
        Command::new(target.join("debug/shop"))
            .envs(dotenv(&repository.join("examples/shop/.env")))
            .env("SOFIE_ADDRESS", address.to_string())
            .current_dir(repository.join("examples/shop"))
            .stdout(Stdio::null())
            .stderr(File::create(&log_path).unwrap())
            .spawn()
            .unwrap(),
    );

    let answer = first_answer(&mut shop.0, address, &log_path);

    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(
        answer.contains("Sign in</") && !answer.contains("identity.action.sign_in"),
        "sofie-identity's `t(…)` keys are written instead of their texts:\n{answer}"
    );
}

struct ServedShop(Child);

impl Drop for ServedShop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn shared_target() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .ancestors()
        .nth(3)
        .unwrap()
        .to_path_buf()
}

fn build_shop(repository: &Path, target: &Path) {
    let cargo = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args([
            "build",
            "--offline",
            "--quiet",
            "--package",
            "shop",
            "--features",
            "sofie/development",
            "--target-dir",
        ])
        .arg(target)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(
        cargo.status.success(),
        "{}",
        String::from_utf8_lossy(&cargo.stderr)
    );
}

fn dotenv(path: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn free_address() -> SocketAddr {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .unwrap()
}

fn first_answer(shop: &mut Child, address: SocketAddr, log_path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = shop.try_wait().unwrap() {
            let log = std::fs::read_to_string(log_path).unwrap();
            panic!("the shop exited with {status} before answering:\n{log}");
        }
        if let Ok(mut stream) = TcpStream::connect(address) {
            stream
                .write_all(b"GET /sign-in HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
                .unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).unwrap();
            return answer;
        }
        assert!(Instant::now() < deadline, "the shop did not answer in 60 s");
        std::thread::sleep(Duration::from_millis(100));
    }
}
