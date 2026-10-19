// Copyright (c) 2026 Mike Grier
//! The near-term load: the output of a command that takes a command line and returns some JSON
//! and an error, as Azure deployment commands do (TW-9.5, TW-D-16).
//!
//! The shape is from the [capture](../../bench-captures/2026-10-17-azure-cli-output-shape.txt),
//! which is of documented samples and not of a run: a long silence, then one burst of indented JSON
//! of a few hundred bytes with the occasional long line on stdout, and errors on stderr, with
//! nothing written between. The reader must deliver the burst at once after the silence, and
//! otherwise leave the process quiet.

use std::time::Duration;

use win_multi_file_tail_watcher::reader::{TailReader, TailReaderOptions};

use crate::harness::{Client, TempDir, crlf, echo, echo_err, gate, script, spawn, wtf};

const STDOUT: u32 = 1;
const STDERR: u32 = 2;

/// The result of a command, as a pretty-printed JSON object. Quotes are balanced and nothing in it
/// is syntax to `cmd.exe`.
const JSON: &[&str] = &[
    "{",
    "  \"fqdns\": \"\",",
    "  \"id\": \"/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/myResourceGroupCLI/providers/Microsoft.Compute/virtualMachines/myVM\",",
    "  \"location\": \"centralus\",",
    "  \"macAddress\": \"00-0D-3A-23-9A-49\",",
    "  \"powerState\": \"VM running\",",
    "  \"privateIpAddress\": \"10.0.0.4\",",
    "  \"publicIpAddress\": \"52.174.34.95\",",
    "  \"resourceGroup\": \"myResourceGroupCLI\",",
    "  \"zones\": \"\"",
    "}",
];

const ERRORS: &[&str] = &[
    "WARNING: This command has been deprecated and will be removed in a future release.",
    "ERROR: QuotaExceeded Operation could not be completed as it results in exceeding approved quota.",
];

fn lines(of: &[&str]) -> Vec<String> {
    of.iter().map(|l| (*l).to_owned()).collect()
}

#[test]
fn a_command_that_is_silent_then_writes_a_burst_is_delivered_at_once_and_the_reader_is_quiet_between()
 {
    let dir = TempDir::new("azure");
    let (out, err) = (dir.path("stdout.log"), dir.path("stderr.log"));

    // Silent until released, then the result on stdout and the errors on stderr.
    let mut parts = vec![gate()];
    parts.extend(JSON.iter().map(|l| echo(l)));
    parts.extend(ERRORS.iter().map(|l| echo_err(l)));
    let mut command = spawn(&script(&parts), &out, Some(&err), true);

    let options = TailReaderOptions::default();
    let reader = TailReader::<u32>::new(&options, 64).expect("a reader");
    let mut client = Client::new(&reader, options.buffer_len);
    let _stdout = reader
        .register(&wtf(&out), 0, STDOUT)
        .expect("register stdout");
    let _stderr = reader
        .register(&wtf(&err), 0, STDERR)
        .expect("register stderr");

    // The silence: polls find nothing, and between them nothing at all happens.
    for _ in 0..3 {
        let polled = client.poll_once();
        assert_eq!(polled.data, 0, "nothing has been written");
        assert!(polled.other.is_empty(), "{:?}", polled.other);
        client.assert_quiet(300);
        std::thread::sleep(Duration::from_millis(200));
    }

    // The burst, and then the command ends.
    command.release();
    assert!(command.wait().success());

    // The first poll after it delivers all of it, from both files, whole.
    let polled = client.poll_once();
    assert!(polled.other.is_empty(), "{:?}", polled.other);
    let (json, error_text) = (crlf(&lines(JSON)), crlf(&lines(ERRORS)));
    assert_eq!(client.assemble(STDOUT).0, json);
    assert_eq!(client.assemble(STDERR).0, error_text);

    // As many reads as the default buffer needs, no more: a burst of a few hundred bytes is a handful.
    let reads = |len: usize| len.div_ceil(options.buffer_len);
    assert_eq!(polled.by_value[&STDOUT], reads(json.len()));
    assert_eq!(polled.by_value[&STDERR], reads(error_text.len()));
    assert!(polled.data < 20, "a handful of reads for a burst this size");

    // And quiet again: nothing more to deliver, and nothing rings.
    let again = client.poll_once();
    assert_eq!(again.data, 0);
    client.assert_quiet(300);
    assert!(reader.take_failure().is_none());
}
