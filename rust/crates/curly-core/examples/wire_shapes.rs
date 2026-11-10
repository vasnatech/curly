//! Ground truth for the HTTP API's wire format: serde's JSON for the
//! `curly-core` types the Tauri bridge hands the frontend, printed one per
//! line so the Ziman port's own encoders can be diffed against them.
//!
//! The bridge's JSON **is** the spec, exactly as the saved-file format is
//! (see `parity.sh`): the React frontend's `types.ts` is a hand-written
//! mirror of it, so it is a second-hand account and not the thing itself.
//! Two shapes here are not guessable from the struct definitions at all --
//! `Duration` is serde's `{"secs","nanos"}` and a `Vec<u8>` is an array of
//! numbers, neither of which is written anywhere in curly's own source.
//!
//! Run: `cargo run -q --example wire_shapes -p curly-core`

use std::time::Duration;

use curly_core::exec::ResponseSummary;
use curly_core::extraction::Extracted;
use curly_core::scenario::{ScenarioRun, StepLog, StepOutcome};

fn main() {
    // A response whose body is NOT valid UTF-8, so `body`'s lossy decoding
    // and `body_bytes`' exactness are visibly different things.
    //
    // The elapsed time is a whole number of MILLISECONDS on purpose, so that
    // the printed line is one the Ziman port can reproduce exactly. That is
    // the one place the two carry different precision: `Duration` holds
    // nanoseconds and Ziman's HTTP client measures in milliseconds, so the
    // port's `nanos` is always a multiple of 1_000_000. The shape is what the
    // frontend reads and the shape is identical; a sub-millisecond difference
    // in a figure the UI prints as "1234 ms" is not observable, and
    // `parity.sh` already normalises an elapsed time because it is a
    // measurement rather than an answer.
    let summary = ResponseSummary {
        status: 201,
        headers: vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("x-request-id".to_string(), "abc-123".to_string()),
        ],
        body: String::from_utf8_lossy(&[0x7b, 0x22, 0x61, 0x22, 0x3a, 0xff, 0x7d]).into_owned(),
        body_bytes: vec![0x7b, 0x22, 0x61, 0x22, 0x3a, 0xff, 0x7d],
        elapsed: Duration::from_millis(1234),
    };
    println!("{}", serde_json::to_string(&summary).unwrap());

    // The empty edges: no headers, an empty body, a zero duration.
    let empty = ResponseSummary {
        status: 204,
        headers: vec![],
        body: String::new(),
        body_bytes: vec![],
        elapsed: Duration::ZERO,
    };
    println!("{}", serde_json::to_string(&empty).unwrap());

    println!(
        "{}",
        serde_json::to_string(&Extracted {
            name: "TOKEN".to_string(),
            value: "s3cret".to_string(),
            secret: true,
        })
        .unwrap()
    );
    println!(
        "{}",
        serde_json::to_string(&Extracted {
            name: "id".to_string(),
            value: "42".to_string(),
            secret: false,
        })
        .unwrap()
    );

    // `StepOutcome` carries no serde attributes at all, so it is serde's
    // DEFAULT externally-tagged enum: a unit variant is the bare string
    // `"Ok"` and a payload variant is `{"Halted": ...}`. Neither spelling is
    // written anywhere in curly's source, and the three variants do not
    // agree on which it is -- which is the whole reason this is printed
    // rather than read off the struct.
    let ok_run = ScenarioRun {
        outcome: StepOutcome::Ok,
        steps: vec![StepLog {
            reference: "auth/login".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/login".to_string(),
            status: Some(200),
            error: None,
            extracted: vec!["TOKEN".to_string()],
        }],
    };
    println!("{}", serde_json::to_string(&ok_run).unwrap());

    // The halted case, and a step that never got a response at all: `status`
    // absent with `error` present, which is the pair that tells a network
    // failure from an HTTP one.
    let halted = ScenarioRun {
        outcome: StepOutcome::Halted("a step returned 500".to_string()),
        steps: vec![StepLog {
            reference: "health/check".to_string(),
            method: "GET".to_string(),
            url: "https://down.example.com/health".to_string(),
            status: None,
            error: Some("connection refused".to_string()),
            extracted: vec![],
        }],
    };
    println!("{}", serde_json::to_string(&halted).unwrap());

    println!(
        "{}",
        serde_json::to_string(&ScenarioRun {
            outcome: StepOutcome::HttpFailure,
            steps: vec![],
        })
        .unwrap()
    );
}
