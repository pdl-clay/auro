// SPDX-License-Identifier: MIT OR Apache-2.0

#![cfg(feature = "hot-reload")]

//! End-to-end proof that a dx-generated patch reaches a live boundary.
//!
//! Set `AURO_TEST_PATCH_APP` to the desktop app produced by `dx build --fat-binary --desktop`
//! (run with a pinned ASLR base) and `AURO_TEST_PATCH_LIB` to the jump-table JSON extracted from
//! `dx tools hotpatch`. The fixture prints one machine-readable line per response; the test drives
//! it over stdin. Without both variables the tests skip loudly: a skip is not a patch proof.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const APP_ENV: &str = "AURO_TEST_PATCH_APP";
const PATCH_ENV: &str = "AURO_TEST_PATCH_LIB";

struct Fixture {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Fixture {
    fn start(app: &Path) -> Self {
        let mut child = Command::new(app)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .env(auro::hot::ENABLE_ENV, "1")
            .spawn()
            .expect("fixture app must start");
        let stdin = child.stdin.take().expect("fixture stdin");
        let stdout = BufReader::new(child.stdout.take().expect("fixture stdout"));
        let mut fixture = Self {
            child,
            stdin,
            stdout,
        };
        fixture.expect("title-one");
        fixture.expect("counter-0");
        fixture.wait_for_prefix("revision-");
        fixture.expect("ready");
        fixture
    }

    fn send(&mut self, command: &str) {
        writeln!(self.stdin, "{command}").expect("fixture stdin");
        self.stdin.flush().expect("fixture stdin");
    }

    fn next(&mut self) -> String {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).expect("fixture stdout");
        assert_ne!(read, 0, "fixture closed stdout before answering");
        line.trim_end().to_owned()
    }

    fn expect(&mut self, expected: &str) {
        let actual = self.next();
        assert_eq!(actual, expected, "unexpected fixture response");
    }

    fn wait_for_prefix(&mut self, prefix: &str) -> String {
        for _ in 0..64 {
            let line = self.next();
            if let Some(value) = line.strip_prefix(prefix) {
                return value.to_owned();
            }
        }
        panic!("fixture did not emit a line starting with {prefix:?}");
    }

    fn patch(&mut self, artifact: &Path) {
        self.send(&format!("patch:{}", artifact.display()));
        self.expect("patch-applied");
        let revision: u64 = self
            .wait_for_prefix("revision-")
            .parse()
            .expect("revision must be an integer");
        assert!(revision > 0, "an applied patch must advance the revision");
        self.expect("title-two");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn artifacts(test: &str) -> Option<(PathBuf, PathBuf)> {
    let app = std::env::var_os(APP_ENV).map(PathBuf::from);
    let patch = std::env::var_os(PATCH_ENV).map(PathBuf::from);
    match (app, patch) {
        (Some(app), Some(patch)) => {
            assert!(app.is_file(), "{APP_ENV} must name the dx fat binary");
            assert!(patch.is_file(), "{PATCH_ENV} must name the jump-table JSON");
            Some((app, patch))
        }
        _ => {
            eprintln!(
                "SKIP {test}: set {APP_ENV} to the dx fat binary and {PATCH_ENV} to the dx \
                 jump-table JSON; without both there is no e2e proof of a real patch"
            );
            None
        }
    }
}

#[test]
fn mounts_real_patch_and_keeps_state() {
    let Some((app, patch)) = artifacts("mounts_real_patch_and_keeps_state") else {
        return;
    };
    let mut fixture = Fixture::start(&app);
    for expected in 1..=3 {
        fixture.send("bump");
        fixture.expect(&format!("counter-{expected}"));
    }
    fixture.send("frame");
    fixture.expect("title-one");
    fixture.patch(&patch);
    fixture.expect("counter-3");
    fixture.send("frame");
    fixture.expect("title-two");
    fixture.send("state");
    fixture.expect("counter-3");
}

#[test]
fn stale_callback_does_not_run_after_apply() {
    let Some((app, patch)) = artifacts("stale_callback_does_not_run_after_apply") else {
        return;
    };
    let mut fixture = Fixture::start(&app);
    fixture.send("register");
    fixture.expect("registered");
    // Before a patch the callback is current, so it is allowed to touch its own state.
    fixture.send("invoke-stale");
    fixture.expect("callback-ran");
    // Register a fresh callback so the state checked after the patch has not already been used.
    fixture.send("register");
    fixture.expect("registered");

    fixture.patch(&patch);
    fixture.expect("counter-0");
    fixture.send("invoke-stale");
    fixture.expect("stale-cleared");
    fixture.expect("old-state-untouched");
    // A substituted `callback-ran` line would appear here; the next line must be the state reply.
    fixture.send("state");
    fixture.expect("counter-0");
}
