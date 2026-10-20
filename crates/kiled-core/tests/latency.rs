//! Typing latency on a large file: an edit, its incremental reparse, and the highlights of a
//! screenful (DESIGN.md §5.4). Run with
//! `cargo test --release -p kiled-core --test latency -- --ignored --nocapture`.

use std::time::Instant;

use kiled_core::{
    document::Document,
    language, ops,
    selection::{Selection, Selections},
    text::Indent,
};

/// A Rust file of about `lines` lines, made of copies of this crate's own sources.
fn large_rust(lines: usize) -> String {
    let sources = [
        include_str!("../src/document.rs"),
        include_str!("../src/ops.rs"),
        include_str!("../src/syntax.rs"),
    ];
    let mut text = String::new();
    while text.lines().count() < lines {
        for source in sources {
            // Inner doc comments may only start a file; keep each copy a valid module.
            text.push_str("mod copy {\n");
            text.push_str(&source.replace("//!", "//"));
            text.push_str("\n}\n");
        }
    }
    text
}

#[test]
#[ignore = "a measurement, run on purpose in release"]
fn typing_in_a_large_file_should_stay_under_a_millisecond_per_key() {
    for size in [2_000, 50_000] {
        measure(size);
    }
}

fn measure(size: usize) {
    let text = large_rust(size);
    let rust = language::for_path(std::path::Path::new("big.rs"));
    let opened = Instant::now();
    let mut doc = Document::new(&text, rust, Indent::default());
    let open = opened.elapsed();
    let lines = doc.rope().len_lines();
    let mut worst = std::time::Duration::ZERO;
    let mut total = std::time::Duration::ZERO;
    let mut editing = std::time::Duration::ZERO;
    // Type a statement on a new line in the middle of the file, a screenful in view around
    // the cursor: the code is briefly invalid at that one place, as it is while typing.
    let line = lines / 2;
    let at = kiled_core::text::line_end(doc.rope(), line);
    doc.set_selections(Selections::single(Selection::cursor(at)));
    ops::newline(&mut doc);
    let typed = "let total = counts.iter().map(|count| count.times).sum::<usize>(); // done";
    let keys = u32::try_from(typed.chars().count()).unwrap_or(1);
    for ch in typed.chars() {
        let started = Instant::now();
        ops::type_char(&mut doc, ch);
        let edited = started.elapsed();
        let highlights = doc.highlights(line.saturating_sub(30)..line + 30);
        let elapsed = started.elapsed();
        assert_ne!(highlights.len(), 0);
        editing += edited;
        worst = worst.max(elapsed);
        total += elapsed;
    }
    let average = total / keys;
    println!(
        "{lines} lines ({} KB): opened and parsed in {open:?}; per key: average {average:?} \
         (edit and reparse {:?}), worst {worst:?}",
        text.len() / 1024,
        editing / keys
    );
    assert!(average.as_micros() < 1000, "average {average:?}");
}
