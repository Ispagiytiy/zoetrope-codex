//! Replay assembly (native).
//!
//! Parse every file on disk up front, merge by timestamp into one ordered
//! [`ReplayItem`] stream, hand it to the App in a single [`UiEvent::ReplayLoaded`]
//! — then keep tailing the file via [`tail_loop`](super::live::tail_loop) (a
//! replayed file that grows "goes live" on its own). Pacing/seeking live in the
//! App's `Timeline`. The portable item + ordering live in [`super::item`].

use std::path::Path;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use crate::transcript::{self, SubagentMeta};

use super::item::{ReplayItem, date_and_sort, entry_timestamp};
use super::live::{LiveSession, SnapshotSeed, resolve_live_target_for_provider, tail_loop};
use super::{Flow, Source, TailRequest, UiEvent, Update, WatchTarget};

/// Replay feeder: parse everything on disk, merge by timestamp, hand the whole
/// (sorted) stream to the App in one [`UiEvent::ReplayLoaded`] — then KEEP TAILING
/// the file. Completion is unknowable (any session can be resumed), so a replayed
/// file is never assumed finished: if it grows, the new appends flow in and the
/// session "goes live" on its own. Pacing/seeking live in the App's `Timeline`.
///
/// Returns [`Flow::Switch`] on a `Watch`, [`Flow::Exit`] if the channel closes.
pub(crate) async fn run_replay(
    target: &WatchTarget,
    ui_tx: &mpsc::Sender<UiEvent>,
    req_rx: &mut mpsc::Receiver<TailRequest>,
    speed: f64,
) -> Flow {
    let path = &target.path;
    let session_id = transcript::session_id_from_path(path);

    // The up-front full parse reads every session file synchronously (the 2MB
    // transcript takes ~0.3s). Run it on a blocking thread so it never stalls a
    // runtime worker — robust even on a single-threaded runtime flavor.
    let owned_path = path.to_path_buf();
    let provider = target.provider;
    let (items, info, seed) =
        match tokio::task::spawn_blocking(move || build_replay_for_provider(&owned_path, provider))
            .await
        {
            Ok(loaded) => loaded,
            // A panic in the parse task would otherwise `unwrap_or_default()` into an
            // empty session indistinguishable from a genuinely empty one — surface it.
            Err(e) => {
                let _ = ui_tx
                    .send(UiEvent::Error(format!("failed to load session: {e}")))
                    .await;
                return Flow::Exit;
            }
        };

    let speed = if speed > 0.0 { speed } else { 1.0 };

    if ui_tx
        .send(UiEvent::ReplayLoaded {
            session_id: session_id.clone(),
            items,
            speed,
            info,
        })
        .await
        .is_err()
    {
        return Flow::Exit;
    }

    // Keep tailing the SAME file for appends. Resolve to (dir, file); a replay
    // target is a concrete file, so disable newer-session auto-switch (you asked
    // for this file) by clearing project_dir. Tail offsets are seeded from the
    // byte positions the bulk parse actually consumed — NOT the current EOF —
    // so lines appended while the bulk parse ran are emitted, not skipped.
    let Some((project_dir, main_path)) = resolve_live_target_for_provider(path, target.provider)
    else {
        // Unreadable target: nothing to tail, just wait for a switch/exit.
        return match req_rx.recv().await {
            Some(TailRequest::Watch(p)) => Flow::Switch(WatchTarget {
                path: p,
                provider: crate::provider::ProviderKind::Auto,
            }),
            Some(TailRequest::WatchWithProvider { path, provider }) => {
                Flow::Switch(WatchTarget { path, provider })
            }
            None => Flow::Exit,
        };
    };
    let mut session = LiveSession::new(project_dir, main_path, target.provider);
    session.project_dir = None; // replay pins one file — no auto-switch
    session.seed(seed);

    tail_loop(session, session_id, ui_tx, req_rx).await
}

/// Build the merged, timestamp-ordered replay item list from all session files,
/// plus the [`SnapshotSeed`] recording how far into each file the parse read
/// (so the follow-up tail resumes exactly there).
///
/// Parsing order within a file is preserved; the global sort is stable so
/// entries with equal (or missing, via predecessor) timestamps keep their
/// relative order. Missing-timestamp entries inherit the previous entry's
/// timestamp *within their own file* before the merge sort, so they ride along
/// with their predecessor.
#[allow(dead_code)]
pub(crate) fn build_replay(
    main_path: &Path,
) -> (Vec<ReplayItem>, crate::state::SessionInfo, SnapshotSeed) {
    build_replay_for_provider(main_path, crate::provider::ProviderKind::Auto)
}

/// Provider-aware replay assembly. The legacy [`build_replay`] wrapper keeps
/// native/browser callers source-compatible while explicit Codex requests can
/// use a non-standard filename containing rollout envelopes.
pub(crate) fn build_replay_for_provider(
    main_path: &Path,
    provider: crate::provider::ProviderKind,
) -> (Vec<ReplayItem>, crate::state::SessionInfo, SnapshotSeed) {
    let mut items: Vec<ReplayItem> = Vec::new();
    let mut seed = SnapshotSeed::default();

    let parse =
        |path: &Path, source: Source, items: &mut Vec<ReplayItem>, seed: &mut SnapshotSeed| {
            let consumed = parse_file_into(path, source, items);
            seed.offsets.insert(path.to_path_buf(), consumed);
        };

    // Main file.
    parse(main_path, Source::Main, &mut items, &mut seed);

    let detected_codex = provider == crate::provider::ProviderKind::Codex
        || (provider == crate::provider::ProviderKind::Auto
            && transcript::is_codex_session_file(main_path));

    // Codex keeps root and child rollouts side by side in a date directory.
    // Only the target and its parent-linked descendants are included.
    if detected_codex {
        let normalized_main_path = transcript::normalize_codex_path(main_path);
        let files = transcript::scan_codex_session_files(main_path);
        let root_id = transcript::codex_session_meta(main_path).map(|meta| meta.session_id);
        for file in files {
            let is_main = file.path == normalized_main_path;
            if !is_main {
                let Some(meta) = &file.meta else { continue };
                let agent_id = meta.session_id.clone();
                let parent = meta
                    .parent_thread_id
                    .as_deref()
                    .filter(|parent| root_id.as_deref() != Some(*parent))
                    .map(|parent| format!("{}{}", transcript::CODEX_PARENT_PREFIX, parent));
                if push_codex_meta_item(&agent_id, parent, meta, &mut items) {
                    // Metadata is embedded in session_meta; use the transcript
                    // path as the virtual sidecar key so live follow does not
                    // emit the same birth marker a second time.
                    seed.seen_meta.insert(file.path.clone());
                }
                parse(&file.path, Source::Sub(agent_id), &mut items, &mut seed);
            } else {
                // The main rollout is already parsed above.
            }
        }
    // All non-main files via the shared Claude transcript layout API (same
    // discovery the live tailer and `inspect` use).
    } else if let Some(subagents) = transcript::subagents_dir(main_path) {
        // Direct subagents.
        for f in transcript::scan_subagent_files(&subagents, None) {
            if f.meta.is_file()
                && push_meta_item(&f.meta, f.agent_id.clone(), f.workflow, &mut items)
            {
                seed.seen_meta.insert(f.meta.clone());
            }
            parse(
                &f.transcript,
                Source::Sub(f.agent_id),
                &mut items,
                &mut seed,
            );
        }
        // Workflow journals + their subagents.
        for wf_id in transcript::scan_workflow_ids(&subagents) {
            let journal = transcript::workflow_journal(&subagents, &wf_id);
            if journal.is_file() {
                parse(
                    &journal,
                    Source::Journal(wf_id.clone()),
                    &mut items,
                    &mut seed,
                );
            }
            let wf_dir = transcript::workflow_dir(&subagents, &wf_id);
            for f in transcript::scan_subagent_files(&wf_dir, Some(&wf_id)) {
                if f.meta.is_file()
                    && push_meta_item(&f.meta, f.agent_id.clone(), f.workflow, &mut items)
                {
                    seed.seen_meta.insert(f.meta.clone());
                }
                parse(
                    &f.transcript,
                    Source::Sub(f.agent_id),
                    &mut items,
                    &mut seed,
                );
            }
        }
    }

    // Route untimed session-level metadata (mode, permission-mode, last-prompt,
    // queue-operation, file-history-snapshot) into the info store and DROP it
    // from the timeline — it isn't activity, and being untimed it would clump at
    // the front. Done here, in file (chronological) order, so latest-wins holds.
    let mut info = crate::state::SessionInfo::default();
    items.retain(|item| match &item.update {
        Update::Entry { entry, .. } if entry.is_timeline_noise() => {
            info.apply(entry);
            false
        }
        _ => true,
    });

    // Date the untimed items (metas/journal/ai-title) and stably sort by ts.
    // Child rollouts may repeat the parent's response prefix. Codex gives
    // those records stable ids; suppress only duplicate ids, never anonymous
    // lines or Claude entries.
    if detected_codex {
        let mut seen = std::collections::HashSet::new();
        items.retain(|item| {
            let Update::Entry { entry, .. } = &item.update else {
                return true;
            };
            let Some(id) = transcript::entry_identity(entry) else {
                return true;
            };
            seen.insert(id.to_owned())
        });
        seed.seen_codex_ids = seen;
    }
    date_and_sort(&mut items);

    (items, info, seed)
}

fn push_codex_meta_item(
    agent_id: &str,
    parent: Option<String>,
    meta: &transcript::CodexSessionMeta,
    items: &mut Vec<ReplayItem>,
) -> bool {
    let description = meta
        .agent_nickname
        .clone()
        .or_else(|| meta.model_provider.clone());
    items.push(ReplayItem::at(
        None,
        Update::SubagentMeta {
            agent_id: agent_id.to_owned(),
            workflow: parent,
            meta: SubagentMeta {
                agent_type: Some("codex".to_owned()),
                description,
                tool_use_id: None,
                stopped_by_user: None,
            },
        },
    ));
    true
}

/// Parse all complete lines of a file into [`ReplayItem`]s, inheriting the
/// previous in-file timestamp for entries that lack one (so they ride along
/// with their predecessor through the merge sort). Returns the number of bytes
/// consumed — up to and including the last newline; a trailing newline-less
/// fragment is a mid-write line, left for the follow-up tail to emit once its
/// newline lands.
fn parse_file_into(path: &Path, source: Source, items: &mut Vec<ReplayItem>) -> u64 {
    let Ok(bytes) = std::fs::read(path) else {
        return 0;
    };
    let consumed = bytes
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let text = String::from_utf8_lossy(&bytes[..consumed]);
    let mut last_ts: Option<DateTime<Utc>> = None;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some(entry) = transcript::parse_line(line) else {
            continue;
        };
        let ts = entry_timestamp(&entry).or(last_ts);
        if ts.is_some() {
            last_ts = ts;
        }
        items.push(ReplayItem::at(
            ts,
            Update::Entry {
                source: source.clone(),
                entry,
            },
        ));
    }
    consumed as u64
}

/// Parse a meta sidecar into a [`ReplayItem`] (no timestamp — leads its agent).
/// Returns whether the meta parsed and was pushed (a mid-write/corrupt sidecar
/// is left for the follow-up tail to retry).
fn push_meta_item(
    path: &Path,
    agent_id: String,
    workflow: Option<String>,
    items: &mut Vec<ReplayItem>,
) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(meta) = serde_json::from_slice::<SubagentMeta>(&bytes) else {
        return false;
    };
    items.push(ReplayItem::at(
        None,
        Update::SubagentMeta {
            agent_id,
            workflow,
            meta,
        },
    ));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_dates_metas_to_first_subagent_entry() {
        use std::io::Write;

        let mut dir = std::env::temp_dir();
        dir.push(format!("zoetrope_replay_order_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session = "22222222-2222-2222-2222-222222222222";
        let sub_dir = dir.join(session).join("subagents");
        std::fs::create_dir_all(&sub_dir).unwrap();

        // Main: one entry BEFORE the subagent starts, one after.
        let main = dir.join(format!("{session}.jsonl"));
        std::fs::File::create(&main)
            .unwrap()
            .write_all(
                concat!(
                    r#"{"type":"user","uuid":"u1","parentUuid":null,"timestamp":"2026-06-05T10:00:00.000Z","message":{"role":"user","content":"start"}}"#, "\n",
                    r#"{"type":"user","uuid":"u3","parentUuid":"u1","timestamp":"2026-06-05T10:02:00.000Z","message":{"role":"user","content":"later"}}"#, "\n",
                )
                .as_bytes(),
            )
            .unwrap();

        // Subagent: first entry at 10:01, with its meta sidecar.
        std::fs::File::create(sub_dir.join("agent-aaaaaaaaaaaaaaaaa.jsonl"))
            .unwrap()
            .write_all(
                concat!(
                    r#"{"type":"user","uuid":"s1","parentUuid":null,"isSidechain":true,"agentId":"aaaaaaaaaaaaaaaaa","timestamp":"2026-06-05T10:01:00.000Z","message":{"role":"user","content":"task"}}"#, "\n",
                )
                .as_bytes(),
            )
            .unwrap();
        std::fs::File::create(sub_dir.join("agent-aaaaaaaaaaaaaaaaa.meta.json"))
            .unwrap()
            .write_all(br#"{"agentType":"guide","toolUseId":"t1"}"#)
            .unwrap();

        let (items, _info, _seed) = build_replay(&main);
        let pos = |pred: &dyn Fn(&ReplayItem) -> bool| items.iter().position(pred);

        let meta_pos = pos(&|i| matches!(&i.update, Update::SubagentMeta { .. })).unwrap();
        let first_main = pos(&|i| {
            matches!(
                &i.update,
                Update::Entry {
                    source: Source::Main,
                    ..
                }
            )
        })
        .unwrap();
        let sub_entry = pos(&|i| {
            matches!(
                &i.update,
                Update::Entry {
                    source: Source::Sub(_),
                    ..
                }
            )
        })
        .unwrap();

        // The meta is dated to the subagent's first entry (10:01): after the
        // 10:00 main entry, before the agent's own entry (tie → meta first).
        assert!(
            items[meta_pos].ts().is_some(),
            "meta must inherit a timestamp"
        );
        assert!(
            first_main < meta_pos,
            "agent must NOT spawn before main starts"
        );
        assert!(
            meta_pos < sub_entry,
            "meta must precede the agent's entries"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_replay_discovers_children_and_deduplicates_parent_prefix() {
        let dir =
            std::env::temp_dir().join(format!("zoetrope-codex-replay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("rollout-root.jsonl");
        let child = dir.join("rollout-child.jsonl");
        let grandchild = dir.join("rollout-grandchild.jsonl");
        let common = r#"{"timestamp":"2026-08-25T00:00:01Z","type":"response_item","payload":{"type":"message","id":"same","role":"assistant","content":[{"type":"output_text","text":"copied"}]}}"#;
        std::fs::write(
            &root,
            format!(
                "{}\n{}\n",
                r#"{"timestamp":"2026-08-25T00:00:00Z","type":"session_meta","payload":{"id":"root","model_provider":"openai"}}"#,
                common
            ),
        )
        .unwrap();
        std::fs::write(
            &child,
            format!(
                "{}\n{}\n{}\n",
                r#"{"timestamp":"2026-08-25T00:00:02Z","type":"session_meta","payload":{"id":"child","parent_thread_id":"root"}}"#,
                common,
                r#"{"timestamp":"2026-08-25T00:00:03Z","type":"event_msg","payload":{"type":"agent_message","message":"child-only"}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            &grandchild,
            format!(
                "{}\n{}\n",
                r#"{"timestamp":"2026-08-25T00:00:04Z","type":"session_meta","payload":{"id":"grandchild","parent_thread_id":"child"}}"#,
                r#"{"timestamp":"2026-08-25T00:00:05Z","type":"event_msg","payload":{"type":"agent_message","message":"nested"}}"#
            ),
        )
        .unwrap();

        let (items, _info, _seed) =
            build_replay_for_provider(&root, crate::provider::ProviderKind::Codex);
        let copied = items
            .iter()
            .filter(|item| matches!(&item.update, Update::Entry { entry: crate::transcript::Entry::Assistant(entry), .. } if entry.envelope.uuid.as_deref() == Some("codex:message:same")))
            .count();
        assert_eq!(copied, 1, "copied parent history appears once");
        assert!(items.iter().any(|item| matches!(
            &item.update,
            Update::SubagentMeta { agent_id, workflow: None, .. } if agent_id == "child"
        )));
        assert!(items.iter().any(|item| matches!(
            &item.update,
            Update::SubagentMeta { agent_id, workflow: Some(parent), .. }
                if agent_id == "grandchild"
                    && parent == &format!("{}child", crate::transcript::CODEX_PARENT_PREFIX)
        )));
        assert!(items.iter().any(|item| matches!(
            &item.update,
            Update::Entry { source: Source::Sub(id), .. } if id == "child"
        )));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_replay_keeps_call_output_and_end_with_shared_call_id() {
        let dir =
            std::env::temp_dir().join(format!("zoetrope-codex-record-kind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("rollout-root.jsonl");
        std::fs::write(
            &root,
            concat!(
                r#"{"timestamp":"2026-08-25T00:00:00Z","type":"session_meta","payload":{"id":"root"}}"#, "\n",
                r#"{"timestamp":"2026-08-25T00:00:01Z","type":"response_item","payload":{"type":"function_call","call_id":"same","name":"exec_command","arguments":"{}"}}"#, "\n",
                r#"{"timestamp":"2026-08-25T00:00:02Z","type":"response_item","payload":{"type":"function_call_output","call_id":"same","output":"ok"}}"#, "\n",
                r#"{"timestamp":"2026-08-25T00:00:03Z","type":"event_msg","payload":{"type":"patch_apply_end","patch_id":"same","success":true,"output":"applied"}}"#, "\n",
            ),
        )
        .unwrap();

        let (items, _info, _seed) =
            build_replay_for_provider(&root, crate::provider::ProviderKind::Codex);
        let identities: Vec<_> = items
            .iter()
            .filter_map(|item| match &item.update {
                Update::Entry { entry, .. } => crate::transcript::entry_identity(entry),
                Update::SubagentMeta { .. } => None,
            })
            .collect();
        assert_eq!(
            identities.len(),
            3,
            "call, output, and end all survive dedup"
        );
        assert!(identities.iter().any(|id| id.contains("function_call:")));
        assert!(
            identities
                .iter()
                .any(|id| id.contains("function_call_output:"))
        );
        assert!(identities.iter().any(|id| id.contains("patch_apply_end:")));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
