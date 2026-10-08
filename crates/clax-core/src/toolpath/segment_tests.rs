//! Segment writer tests (spec §7, §15 "Appender"): an in-memory
//! [`MemFs`] and a [`ManualClock`], no sleeps. The golden history's rows
//! are the events; the journal's conformance to Toolpath's schema is
//! `segments_seal_and_validate_against_schema` in `tests.rs`.

use super::*;
use crate::toolpath::tests::{History, history, segment};
use crate::working::ManualClock;

const DIR: &str = "/home/toolpath/journal";
const COMMIT: &str = "abc1234def5678abc1234def5678abc1234def56";

fn cfg(h: &History) -> SegmentConfig {
    SegmentConfig::new(h.env.install.clone(), "0.3.1", COMMIT)
}

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::at("2026-10-06T12:00:00Z"))
}

fn writer(fs: &MemFs, cfg: SegmentConfig, clock: Arc<ManualClock>) -> SegmentWriter {
    SegmentWriter::open_or_recover(DIR, cfg, clock, Box::new(fs.clone())).unwrap()
}

fn seg_path(day: &str, nnn: u32) -> PathBuf {
    PathBuf::from(DIR)
        .join(&day[..4])
        .join(&day[4..6])
        .join(format!("clax-6a1f0c3e-{day}-{nnn:03}.path.jsonl"))
}

/// The golden history's rows, the first `on_first` of them on 2026-10-06
/// and the rest moved to the days after, `per_day` to a day.
fn rows_over_days(h: &History, on_first: usize, per_day: usize) -> Vec<AuditRow> {
    h.rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut r = r.clone();
            if i >= on_first {
                let day = 7 + (i - on_first) / per_day;
                r.at = r.at.replace("2026-10-06", &format!("2026-10-{day:02}"));
            }
            r
        })
        .collect()
}

/// Appends `rows` in batches of `batch`, as the appender does: each batch
/// is the rows past the writer's cursor.
fn append_all(w: &mut SegmentWriter, rows: &[AuditRow], batch: usize) {
    loop {
        let cursor = w.cursor().unwrap_or(0);
        let next: Vec<AuditRow> = rows
            .iter()
            .filter(|r| r.seq > cursor)
            .take(batch)
            .cloned()
            .collect();
        if next.is_empty() {
            return;
        }
        w.append_batch(&next).unwrap();
    }
}

/// Every file's contents.
fn snapshot(fs: &MemFs) -> BTreeMap<PathBuf, Vec<u8>> {
    fs.state().files.clone()
}

fn lines(text: &str) -> Vec<Value> {
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn first_start_writes_path_open_and_backfill() {
    let h = history();
    assert!(
        h.rows.iter().any(|r| r.backfilled),
        "the history has backfilled rows"
    );
    let fs = MemFs::new();
    let mut w = writer(&fs, cfg(&h), clock());
    assert_eq!(
        w.cursor(),
        Some(0),
        "an empty journal starts before the first event"
    );
    assert_eq!(w.segment(), None);
    w.append_batch(&h.rows).unwrap();
    let path = seg_path("20261006", 1);
    assert_eq!(fs.paths(), vec![path.clone()]);
    // The backfilled history and what followed it, in one open segment of
    // the shape §7.2 gives.
    assert_eq!(fs.text(&path).unwrap(), segment(&h, false));
    assert_eq!(w.cursor(), Some(h.rows.last().unwrap().seq));
    assert_eq!(w.segment(), Some("clax-6a1f0c3e-20261006-001.path.jsonl"));
    let st = fs.state();
    assert_eq!(st.modes[&path], 0o600);
    for d in [
        DIR,
        "/home/toolpath/journal/2026",
        "/home/toolpath/journal/2026/10",
    ] {
        assert_eq!(st.dirs[Path::new(d)], 0o700, "{d}");
        assert!(st.dir_syncs[Path::new(d)] >= 1, "{d} was fsynced");
    }
    assert!(st.dir_syncs[Path::new("/home/toolpath")] >= 1);
}

#[test]
fn rotates_at_utc_day() {
    let h = history();
    let mut rows = rows_over_days(&h, 30, 20);
    // The clock stepped back: an event of an earlier day stays in the open
    // segment.
    rows[35].at = rows[35].at.replace("2026-10-07", "2026-10-06");
    let fs = MemFs::new();
    let mut w = writer(&fs, cfg(&h), clock());
    w.append_batch(&rows[..10]).unwrap();
    w.append_batch(&rows[10..]).unwrap();
    let (a, b, c) = (
        seg_path("20261006", 1),
        seg_path("20261007", 1),
        seg_path("20261008", 1),
    );
    assert_eq!(fs.paths(), vec![a.clone(), b.clone(), c.clone()]);
    let first = lines(&fs.text(&a).unwrap());
    let n = first.len();
    assert_eq!(
        first[n - 2],
        json!({"Head": {"step_id": step_id(rows[29].seq)}})
    );
    assert_eq!(first[n - 1], json!({"PathClose": {}}));
    assert!(
        fs.state().syncs[&a] >= 1,
        "a segment is synced as it closes"
    );
    let second = lines(&fs.text(&b).unwrap());
    let open = &second[0]["PathOpen"];
    assert_eq!(open["id"], "clax-journal-6a1f0c3e-20261007-001");
    assert_eq!(open["meta"]["title"], "Clax audit trail 2026-10-07 #1");
    assert_eq!(open["meta"]["clax"]["first_seq"], rows[30].seq);
    assert_eq!(
        open["meta"]["refs"],
        json!([{"rel": "continues", "href": "clax-6a1f0c3e-20261006-001.path.jsonl"}])
    );
    let steps: Vec<&Value> = second.iter().filter_map(|l| l.get("Step")).collect();
    assert!(steps[0]["step"].get("parents").is_none());
    assert_eq!(steps.len(), 20, "the stepped-back event stayed");
    // Every segment re-declares the actors it names.
    for p in [&a, &b, &c] {
        let ls = lines(&fs.text(p).unwrap());
        let defined: Vec<&str> = ls
            .iter()
            .filter_map(|l| l.pointer("/ActorDef/actor").and_then(Value::as_str))
            .collect();
        for s in ls.iter().filter_map(|l| l.get("Step")) {
            assert!(defined.contains(&s["step"]["actor"].as_str().unwrap()));
        }
    }
    // A new segment on a day that already has one counts on.
    w.close().unwrap();
    let mut more = rows[61].clone();
    more.seq = 100;
    w.append_batch(&[more]).unwrap();
    assert!(fs.text(&seg_path("20261008", 2)).is_some());
}

#[test]
fn rotates_at_size_cap() {
    let h = history();
    let small = SegmentConfig {
        max_bytes: 12 << 10,
        ..cfg(&h)
    };
    let fs = MemFs::new();
    let mut w = writer(&fs, small.clone(), clock());
    append_all(&mut w, &h.rows, 512);
    let paths = fs.paths();
    assert!(paths.len() >= 3, "{paths:?}");
    for (i, p) in paths.iter().enumerate() {
        assert_eq!(*p, seg_path("20261006", i as u32 + 1));
        let text = fs.text(p).unwrap();
        let steps = text.lines().filter(|l| l.starts_with("{\"Step\"")).count();
        assert!(
            text.len() as u64 <= small.max_bytes || steps == 1,
            "{} is {} bytes",
            p.display(),
            text.len()
        );
    }
    // Where a segment splits depends on the rows alone, not on batching.
    let one_by_one = MemFs::new();
    let mut w = writer(&one_by_one, small, clock());
    append_all(&mut w, &h.rows, 1);
    assert_eq!(snapshot(&one_by_one), snapshot(&fs));
}

#[test]
fn recovery_truncates_partial_line() {
    let h = history();
    let fs = MemFs::new();
    let mut w = writer(&fs, cfg(&h), clock());
    w.append_batch(&h.rows[..10]).unwrap();
    drop(w);
    let path = seg_path("20261006", 1);
    fs.state()
        .files
        .get_mut(&path)
        .unwrap()
        .extend_from_slice(b"{\"Step\":{\"chan");
    let mut w = writer(&fs, cfg(&h), clock());
    assert_eq!(w.cursor(), Some(h.rows[9].seq));
    assert!(fs.text(&path).unwrap().ends_with("}\n"));
    w.append_batch(&h.rows[10..]).unwrap();
    assert_eq!(fs.text(&path).unwrap(), segment(&h, false));
}

/// The journal of `rows` written in one go, for comparing a crashed and
/// resumed one with.
fn reference(h: &History, rows: &[AuditRow], cfg: &SegmentConfig) -> BTreeMap<PathBuf, Vec<u8>> {
    let fs = MemFs::new();
    let mut w = writer(&fs, cfg.clone(), clock());
    append_all(&mut w, rows, 7);
    let _ = h;
    snapshot(&fs)
}

#[test]
fn crash_mid_batch_resumes_without_duplicates() {
    let h = history();
    let rows = rows_over_days(&h, 25, 25);
    let cfg = SegmentConfig {
        max_bytes: 20 << 10,
        ..cfg(&h)
    };
    let want = reference(&h, &rows, &cfg);
    assert!(want.len() >= 3, "the journal rotates");
    let files: Vec<(&PathBuf, &Vec<u8>)> = want.iter().collect();
    // A crash leaves the segments before the one being written whole, and
    // that one cut anywhere: at a line's start or end, a byte in, or inside
    // it. Each resumes to the same bytes.
    let mut cuts = 0;
    for k in 0..files.len() {
        let bytes = files[k].1;
        let mut at: Vec<usize> = vec![0, bytes.len()];
        let mut start = 0;
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'\n' {
                let end = i + 1;
                at.extend([start + 1, (start + end) / 2, end - 1, end]);
                start = end;
            }
        }
        at.sort_unstable();
        at.dedup();
        for c in at {
            let fs = MemFs::new();
            {
                let mut st = fs.state();
                for (p, b) in &files[..k] {
                    st.files.insert((*p).clone(), (*b).clone());
                }
                st.files.insert(files[k].0.clone(), bytes[..c].to_vec());
                for p in files.iter().take(k + 1).map(|f| f.0) {
                    let mut d = p.parent();
                    while let Some(x) = d {
                        st.dirs.insert(x.to_path_buf(), 0o700);
                        d = x.parent();
                    }
                }
            }
            let mut w = writer(&fs, cfg.clone(), clock());
            append_all(&mut w, &rows, 512);
            let got = snapshot(&fs);
            if got != want {
                for (p, b) in &want {
                    let g = got.get(p).cloned().unwrap_or_default();
                    if &g != b {
                        let i = g
                            .iter()
                            .zip(b.iter())
                            .position(|(x, y)| x != y)
                            .unwrap_or(g.len().min(b.len()));
                        eprintln!(
                            "{}: got {} want {} first diff {i}\n got: {}\nwant: {}",
                            p.display(),
                            g.len(),
                            b.len(),
                            String::from_utf8_lossy(
                                &g[i.saturating_sub(200)..(i + 200).min(g.len())]
                            ),
                            String::from_utf8_lossy(
                                &b[i.saturating_sub(200)..(i + 200).min(b.len())]
                            )
                        );
                    }
                }
                panic!(
                    "resuming from {} cut at byte {c} differs",
                    files[k].0.display()
                );
            }
            cuts += 1;
        }
    }
    assert!(cuts > 300, "{cuts} cuts");

    // A write cut short in the writer itself: the same writer retries the
    // batch, and a restarted one resumes, each to the same bytes.
    for restart in [false, true] {
        let fs = MemFs::new();
        let mut w = writer(&fs, cfg.clone(), clock());
        w.append_batch(&rows[..12]).unwrap();
        fs.fail(&["append"], 1, 5);
        fs.state().partial = Some(700);
        assert!(w.append_batch(&rows[12..30]).is_err());
        if restart {
            drop(w);
            w = writer(&fs, cfg.clone(), clock());
        }
        append_all(&mut w, &rows, 9);
        assert!(snapshot(&fs) == want, "restart {restart}");
    }
}

#[test]
fn damaged_segment_is_renamed_not_deleted() {
    let h = history();
    let fs = MemFs::new();
    let mut w = writer(&fs, cfg(&h), clock());
    w.append_batch(&h.rows[..10]).unwrap();
    drop(w);
    let path = seg_path("20261006", 1);
    let edited = {
        let mut st = fs.state();
        let f = st.files.get_mut(&path).unwrap();
        f.extend_from_slice(b"{\"note\":\"added by hand\"}\n");
        f.clone()
    };
    let mut w = writer(&fs, cfg(&h), clock());
    let damaged = path.with_file_name("clax-6a1f0c3e-20261006-001.path.jsonl.damaged");
    assert_eq!(
        fs.state().files.get(&damaged),
        Some(&edited),
        "renamed, bytes kept"
    );
    assert!(fs.text(&path).is_none());
    // The journal goes on after the highest seq that parses.
    assert_eq!(w.cursor(), Some(h.rows[9].seq));
    w.append_batch(&h.rows[10..]).unwrap();
    let next = lines(&fs.text(&seg_path("20261006", 2)).unwrap());
    assert_eq!(
        next[0]["PathOpen"]["meta"]["clax"]["first_seq"],
        h.rows[10].seq
    );
    assert_eq!(
        next[0]["PathOpen"]["meta"]["refs"],
        json!([{"rel": "continues", "href": "clax-6a1f0c3e-20261006-001.path.jsonl.damaged"}])
    );

    // A line edited in the middle is damage too; a second damaged copy of
    // one name keeps the first.
    drop(w);
    let second = seg_path("20261006", 2);
    {
        let mut st = fs.state();
        let f = st.files.get_mut(&second).unwrap();
        let text = String::from_utf8(f.clone()).unwrap();
        let mut ls: Vec<&str> = text.lines().collect();
        ls[3] = "not json";
        *f = (ls.join("\n") + "\n").into_bytes();
    }
    let w = writer(&fs, cfg(&h), clock());
    assert!(
        fs.text(&second.with_file_name("clax-6a1f0c3e-20261006-002.path.jsonl.damaged"))
            .is_some()
    );
    assert!(
        fs.text(&damaged).is_some(),
        "the first damaged file is kept"
    );
    assert_eq!(w.cursor(), Some(h.rows.last().unwrap().seq));
}

#[test]
fn eio_on_append_rewrites_the_same_batch() {
    let h = history();
    let fs = MemFs::new();
    let mut w = writer(&fs, cfg(&h), clock());
    w.append_batch(&h.rows[..5]).unwrap();
    for (errno, partial) in [(5, Some(300)), (28, None), (13, Some(1))] {
        fs.fail(&["append"], 1, errno);
        fs.state().partial = partial;
        let e = w.append_batch(&h.rows[5..20]).unwrap_err();
        assert_eq!(e.raw_os_error(), Some(errno));
    }
    w.append_batch(&h.rows[5..]).unwrap();
    assert_eq!(
        fs.text(&seg_path("20261006", 1)).unwrap(),
        segment(&h, false)
    );
    // A failure to make the directory or the file is retried the same way.
    let fs = MemFs::new();
    let mut w = writer(&fs, cfg(&h), clock());
    fs.fail(&["create_dir_all"], 1, 13);
    assert!(w.append_batch(&h.rows).is_err());
    fs.fail(&["create"], 1, 28);
    assert!(w.append_batch(&h.rows).is_err());
    w.append_batch(&h.rows).unwrap();
    assert_eq!(
        fs.text(&seg_path("20261006", 1)).unwrap(),
        segment(&h, false)
    );
}

#[test]
fn fsync_is_coalesced() {
    let h = history();
    let fs = MemFs::new();
    let clock = clock();
    let mut w = writer(&fs, cfg(&h), clock.clone());
    let path = seg_path("20261006", 1);
    let syncs = |fs: &MemFs| fs.state().syncs.get(&path).copied().unwrap_or(0);
    assert!(!w.sync_if_due().unwrap(), "nothing to sync");
    // A steady stream of batches is synced once every five seconds, never
    // per batch.
    for (i, r) in h.rows[..20].iter().enumerate() {
        w.append_batch(std::slice::from_ref(r)).unwrap();
        w.sync_if_due().unwrap();
        if i % 5 == 4 {
            clock.advance(1);
        }
    }
    assert_eq!(syncs(&fs), 0, "four seconds of batches: none synced yet");
    clock.advance(1);
    w.append_batch(&h.rows[20..21]).unwrap();
    assert!(w.sync_if_due().unwrap(), "five seconds unsynced");
    assert_eq!(syncs(&fs), 1);
    assert!(!w.unsynced());
    // A quiet moment syncs at once.
    w.append_batch(&h.rows[21..22]).unwrap();
    assert!(!w.sync_if_due().unwrap());
    w.sync().unwrap();
    assert_eq!(syncs(&fs), 2);
    w.sync().unwrap();
    assert_eq!(syncs(&fs), 2, "nothing new to sync");
    // A sync that fails leaves the bytes unsynced, and the writer reads its
    // segment back before it writes again.
    w.append_batch(&h.rows[22..23]).unwrap();
    fs.fail(&["sync_data"], 1, 5);
    assert!(w.sync().is_err());
    assert!(w.unsynced());
    assert_eq!(w.cursor(), None, "the segment is read back first");
    w.append_batch(&h.rows[23..32]).unwrap();
    assert_eq!(w.cursor(), Some(h.rows[31].seq));
    // Shutdown syncs whatever is unsynced as it closes.
    w.append_batch(&h.rows[32..]).unwrap();
    let before = syncs(&fs);
    w.close().unwrap();
    assert_eq!(syncs(&fs), before + 1);
    assert_eq!(
        fs.text(&path).unwrap(),
        segment(&h, true),
        "the read-back changed nothing"
    );
}

#[test]
fn retain_days_removes_whole_old_segments_only() {
    let h = history();
    // Ten rows a day from 2026-10-06 to 2026-10-12.
    let rows = rows_over_days(&h, 10, 10);
    let keep = SegmentConfig {
        retain_days: 2,
        ..cfg(&h)
    };
    // The clock is at 2026-10-12: the cutoff is 2026-10-10.
    let clock = Arc::new(ManualClock::at("2026-10-12T12:00:00Z"));
    let fs = MemFs::new();
    let mut w = writer(&fs, keep.clone(), clock.clone());
    w.append_batch(&rows[..15]).unwrap();
    // Events before the cutoff are never written: no file of theirs is
    // created, and the cursor still passes them.
    assert!(fs.state().created.is_empty(), "{:?}", fs.state().created);
    assert_eq!(w.cursor(), Some(rows[14].seq));
    w.append_batch(&rows[15..]).unwrap();
    assert_eq!(
        fs.state().created,
        vec![
            seg_path("20261010", 1),
            seg_path("20261011", 1),
            seg_path("20261012", 1),
        ]
    );
    // Days later, the closed segments wholly before the new cutoff go, at
    // the next open; a damaged file and the open segment stay.
    let damaged = seg_path("20261010", 9).with_extension("jsonl.damaged");
    fs.state().files.insert(damaged.clone(), b"x\n".to_vec());
    clock.advance(2 * 86_400);
    let mut late = rows[61].clone();
    late.seq = 100;
    late.at = late.at.replace("2026-10-12", "2026-10-14");
    w.append_batch(&[late]).unwrap();
    assert_eq!(
        fs.paths(),
        vec![damaged, seg_path("20261012", 1), seg_path("20261014", 1)]
    );
    let closed = fs.text(&seg_path("20261012", 1)).unwrap();
    assert!(closed.ends_with("{\"PathClose\":{}}\n"), "whole");
    // A segment holding an event of an earlier day (the clock stepped
    // back) is judged by its own day: every event in it is on that day or
    // before.
    let fs = MemFs::new();
    let clock = Arc::new(ManualClock::at("2026-10-12T12:00:00Z"));
    let mut w = writer(&fs, keep.clone(), clock.clone());
    let mut stepped = rows[50..60].to_vec();
    stepped[5].at = stepped[5].at.replace("2026-10-11", "2026-10-10");
    w.append_batch(&stepped).unwrap();
    assert_eq!(fs.paths(), vec![seg_path("20261011", 1)]);
    let text = fs.text(&seg_path("20261011", 1)).unwrap();
    assert_eq!(
        text.lines().filter(|l| l.starts_with("{\"Step\"")).count(),
        10
    );
    // Retention is off at 0.
    let fs = MemFs::new();
    let mut w = writer(&fs, cfg(&h), clock);
    w.append_batch(&rows).unwrap();
    assert_eq!(fs.paths().len(), 7);
}

#[test]
fn a_segment_retention_cannot_remove_does_not_stop_the_journal() {
    let h = history();
    let rows = rows_over_days(&h, 10, 10);
    let keep = SegmentConfig {
        retain_days: 1,
        ..cfg(&h)
    };
    let clock = Arc::new(ManualClock::at("2026-10-08T12:00:00Z"));
    let fs = MemFs::new();
    let mut w = writer(&fs, keep, clock.clone());
    w.append_batch(&rows[..30]).unwrap();
    clock.advance(3 * 86_400);
    fs.fail(&["remove"], 100, 13);
    w.append_batch(&rows[30..]).unwrap();
    assert_eq!(w.cursor(), Some(rows[61].seq), "every event journalled");
    assert!(
        w.warning().unwrap().contains("os error 13"),
        "{:?}",
        w.warning()
    );
    assert!(
        fs.text(&seg_path("20261007", 1)).is_some(),
        "kept, not half-removed"
    );
    // Once removal works again, the next open clears the warning.
    fs.state().fail_count = 0;
    let mut more = rows[61].clone();
    more.seq = 100;
    more.at = more.at.replace("2026-10-12", "2026-10-13");
    w.append_batch(&[more]).unwrap();
    assert_eq!(w.warning(), None);
    assert!(fs.text(&seg_path("20261007", 1)).is_none());
}

#[test]
fn a_segment_resumes_under_the_options_it_was_written_with() {
    let h = history();
    let with_text = cfg(&h);
    let want = reference(&h, &h.rows, &with_text);
    let path = seg_path("20261006", 1);
    // Cut mid-batch, then restarted with `journal_text = false` and a
    // smaller cap: the lost lines come back as they were written.
    let fs = MemFs::new();
    let mut w = writer(&fs, with_text.clone(), clock());
    w.append_batch(&h.rows[..30]).unwrap();
    drop(w);
    let full = want[&path].clone();
    let cut = full.len() - 300;
    {
        let mut st = fs.state();
        let f = st.files.get_mut(&path).unwrap();
        let keep = f.len().min(cut).min(f.len() - 200);
        f.truncate(keep);
    }
    let no_text = SegmentConfig {
        redaction: Redaction {
            no_text: true,
            ..Redaction::NONE
        },
        max_bytes: 32 << 20,
        ..with_text
    };
    // The restart is at 14:05, inside the history's 14:00-14:07: events
    // from 14:05 on were recorded after it.
    let restart = Arc::new(ManualClock::at("2026-10-06T14:05:00Z"));
    let mut w = writer(&fs, no_text.clone(), restart);
    let resumed_at = w.cursor().unwrap();
    // The rows the crash lost are re-rendered with text, into the same
    // segment; the first row after them opens a segment under the new
    // options.
    let lost: Vec<AuditRow> = h.rows[..30]
        .iter()
        .filter(|r| r.seq > resumed_at)
        .cloned()
        .collect();
    assert!(!lost.is_empty());
    w.append_batch(&lost).unwrap();
    let resumed = fs.text(&path).unwrap();
    assert!(
        want[&path].starts_with(resumed.as_bytes()),
        "byte-identical so far"
    );
    w.append_batch(&h.rows[30..]).unwrap();
    let first = lines(&fs.text(&path).unwrap());
    assert_eq!(first.last().unwrap(), &json!({"PathClose": {}}));
    let second = lines(&fs.text(&seg_path("20261006", 2)).unwrap());
    let clax = &second[0]["PathOpen"]["meta"]["clax"];
    assert_eq!(clax["redaction"], json!(["no-text"]));
    assert_eq!(clax["segment_max_bytes"], 32u64 << 20);
    // The new segment begins at the first event recorded since the
    // restart.
    let first_new = h
        .rows
        .iter()
        .find(|r| r.at.as_str() >= "2026-10-06T14:05")
        .unwrap();
    assert!(
        first_new.seq > h.rows[29].seq,
        "the test cuts before the restart time"
    );
    assert_eq!(clax["first_seq"], first_new.seq);
    let first_clax = &first[0]["PathOpen"]["meta"]["clax"];
    assert_eq!(first_clax["redaction"], json!([]));
    assert_eq!(first_clax["segment_max_bytes"], 64u64 << 20);
}

#[test]
fn an_overlong_line_is_damage_and_is_never_held_whole() {
    let h = history();
    let fs = MemFs::new();
    let mut w = writer(&fs, cfg(&h), clock());
    w.append_batch(&h.rows[..10]).unwrap();
    drop(w);
    let path = seg_path("20261006", 1);
    // A line past the 16 MiB cap, newline-terminated: damage.
    let mut long = vec![b'x'; (16 << 20) + 10];
    long.push(b'\n');
    fs.state()
        .files
        .get_mut(&path)
        .unwrap()
        .extend_from_slice(&long);
    let w = writer(&fs, cfg(&h), clock());
    assert_eq!(w.cursor(), Some(h.rows[9].seq));
    assert!(
        fs.text(&path.with_file_name("clax-6a1f0c3e-20261006-001.path.jsonl.damaged"))
            .is_some()
    );
    // Unterminated at the end: a partial line, truncated.
    drop(w);
    let second = seg_path("20261006", 2);
    let mut w = writer(&fs, cfg(&h), clock());
    w.append_batch(&h.rows[10..20]).unwrap();
    drop(w);
    let before = fs.text(&second).unwrap();
    fs.state()
        .files
        .get_mut(&second)
        .unwrap()
        .extend_from_slice(&long[..long.len() - 1]);
    let w = writer(&fs, cfg(&h), clock());
    assert_eq!(fs.text(&second).unwrap(), before);
    assert_eq!(w.cursor(), Some(h.rows[19].seq));
}

#[test]
fn journal_directory_removed_goes_on_in_a_new_segment() {
    let h = history();
    let fs = MemFs::new();
    let mut w = writer(&fs, cfg(&h), clock());
    w.append_batch(&h.rows[..10]).unwrap();
    {
        let mut st = fs.state();
        st.files.clear();
        st.dirs.clear();
    }
    w.append_batch(&h.rows[10..]).unwrap();
    // A new name, after the gone one, continuing it, from the cursor on.
    assert_eq!(fs.paths(), vec![seg_path("20261006", 2)]);
    let ls = lines(&fs.text(&seg_path("20261006", 2)).unwrap());
    let open = &ls[0]["PathOpen"];
    assert_eq!(open["meta"]["clax"]["first_seq"], h.rows[10].seq);
    assert_eq!(
        open["meta"]["refs"][0]["href"],
        "clax-6a1f0c3e-20261006-001.path.jsonl"
    );
}

#[test]
fn recovery_reads_closed_and_half_closed_segments() {
    let h = history();
    let fs = MemFs::new();
    let mut w = writer(&fs, cfg(&h), clock());
    w.append_batch(&h.rows[..10]).unwrap();
    w.close().unwrap();
    // Closed: the cursor comes from its last step; the next event opens a
    // new segment.
    let w = writer(&fs, cfg(&h), clock());
    assert_eq!(w.cursor(), Some(h.rows[9].seq));
    drop(w);
    // `Head` without `PathClose` (a close cut short) is finished.
    let path = seg_path("20261006", 1);
    let whole = fs.text(&path).unwrap();
    let headed = whole.strip_suffix("{\"PathClose\":{}}\n").unwrap();
    fs.state()
        .files
        .insert(path.clone(), headed.as_bytes().to_vec());
    let mut w = writer(&fs, cfg(&h), clock());
    assert_eq!(fs.text(&path).unwrap(), whole);
    w.append_batch(&h.rows[10..]).unwrap();
    assert!(fs.text(&seg_path("20261006", 2)).is_some());
    drop(w);
    // An empty newest segment is removed, and `PathOpen` alone resumes at
    // its first event.
    let third = seg_path("20261006", 3);
    fs.state().files.insert(third.clone(), Vec::new());
    let w = writer(&fs, cfg(&h), clock());
    assert!(fs.text(&third).is_none());
    assert_eq!(w.cursor(), Some(h.rows.last().unwrap().seq));
    let second = seg_path("20261006", 2);
    let first_line = fs
        .text(&second)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string()
        + "\n";
    fs.state().files.insert(second, first_line.into_bytes());
    let w = writer(&fs, cfg(&h), clock());
    assert_eq!(w.cursor(), Some(h.rows[10].seq - 1));
}

/// The real file system: modes, the directory fsync path, a rename that
/// never replaces, and no file with a second link.
#[test]
fn std_fs_makes_private_files_and_never_links() {
    use std::os::unix::fs::MetadataExt;
    let h = history();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("toolpath/journal");
    let mut w = SegmentWriter::open_or_recover(
        &dir,
        SegmentConfig {
            max_bytes: 16 << 10,
            ..cfg(&h)
        },
        clock(),
        Box::new(StdFs::new()),
    )
    .unwrap();
    w.append_batch(&h.rows[..40]).unwrap();
    w.sync().unwrap();
    // A crash's partial line, then a restart.
    let first = dir.join("2026/10/clax-6a1f0c3e-20261006-001.path.jsonl");
    let open = std::fs::read_dir(dir.join("2026/10"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .max()
        .unwrap();
    let before = std::fs::read(&open).unwrap();
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&open)
            .unwrap();
        f.write_all(b"{\"Step\":").unwrap();
    }
    drop(w);
    let mut w = SegmentWriter::open_or_recover(
        &dir,
        SegmentConfig {
            max_bytes: 16 << 10,
            ..cfg(&h)
        },
        clock(),
        Box::new(StdFs::new()),
    )
    .unwrap();
    assert_eq!(std::fs::read(&open).unwrap(), before);
    w.append_batch(&h.rows[40..]).unwrap();
    w.close().unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().mode() & 0o777;
    for d in [
        tmp.path().join("toolpath"),
        dir.clone(),
        dir.join("2026"),
        dir.join("2026/10"),
    ] {
        assert_eq!(mode(&d), 0o700, "{}", d.display());
    }
    for e in std::fs::read_dir(dir.join("2026/10")).unwrap() {
        let p = e.unwrap().path();
        assert_eq!(mode(&p), 0o600, "{}", p.display());
        assert_eq!(std::fs::metadata(&p).unwrap().nlink(), 1, "{}", p.display());
    }
    let mut fs = StdFs::new();
    let other = dir.join("2026/10/other");
    std::fs::write(&other, b"x").unwrap();
    let e = fs.rename(&first, &other).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
    assert!(first.exists());
}

#[test]
fn the_restart_time_is_taken_once_before_the_scan() {
    let h = history();
    let fs = MemFs::new();
    let mut w = writer(&fs, cfg(&h), clock());
    w.append_batch(&h.rows[..30]).unwrap();
    drop(w);
    // Restarted at 14:05 with new options; a failed write at 14:06 makes it
    // read the journal back again.
    let no_text = SegmentConfig {
        redaction: Redaction {
            no_text: true,
            ..Redaction::NONE
        },
        ..cfg(&h)
    };
    let restart = Arc::new(ManualClock::at("2026-10-06T14:05:00Z"));
    let mut w = writer(&fs, no_text, restart.clone());
    // The backlog before 14:05 continues the old segment.
    let before: Vec<AuditRow> = h.rows[30..]
        .iter()
        .filter(|r| r.at.as_str() < "2026-10-06T14:05")
        .cloned()
        .collect();
    w.append_batch(&before).unwrap();
    restart.advance(60);
    fs.fail(&["append"], 1, 5);
    let after: Vec<AuditRow> = h.rows[30..]
        .iter()
        .filter(|r| r.at.as_str() >= "2026-10-06T14:05")
        .cloned()
        .collect();
    assert!(w.append_batch(&after).is_err());
    // The read-back at 14:06 keeps 14:05 as the restart: events from 14:05
    // on still begin the new segment.
    w.append_batch(&after).unwrap();
    let second = lines(&fs.text(&seg_path("20261006", 2)).unwrap());
    let clax = &second[0]["PathOpen"]["meta"]["clax"];
    assert_eq!(clax["redaction"], json!(["no-text"]));
    assert_eq!(clax["first_seq"], after[0].seq);
}
