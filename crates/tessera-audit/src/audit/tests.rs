use super::*;

type Entry = AuditEntry<String, String, String>;

fn entry(seq: u64, value: &str) -> Entry {
    Entry {
        seq,
        ts_mono_ms: seq,
        origin: "actor".into(),
        mutation: value.into(),
        effect: "applied".into(),
    }
}

fn test_options() -> AuditStoreOptions {
    AuditStoreOptions {
        min_free_bytes: 0,
        ..AuditStoreOptions::default()
    }
}

fn open_store(
    path: impl Into<PathBuf>,
) -> Result<(ChainedEventStore<Entry>, Vec<Entry>), AuditError> {
    ChainedEventStore::open_with_options(path, DEFAULT_CAPACITY, test_options())
}

#[test]
fn live_projection_detects_gaps_and_eviction() {
    let mut log = AuditLog::new(2);
    log.push_verified(entry(1, "one")).unwrap();
    log.push_verified(entry(2, "two")).unwrap();
    log.push_verified(entry(3, "three")).unwrap();
    assert_eq!(log.oldest_seq(), 2);
    assert_eq!(log.latest_seq(), 3);
    assert_eq!(log.since(1).entries.len(), 2);
    assert!(matches!(
        log.push_verified(entry(5, "gap")),
        Err(AuditError::Sequence {
            expected: 4,
            actual: 5
        })
    ));
}

#[test]
fn durable_store_round_trips_and_continues_the_chain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit/events.jsonl");
    let (mut store, replayed) = open_store(&path).unwrap();
    assert!(replayed.is_empty());
    store.append(&entry(1, "one")).unwrap();
    store.append(&entry(2, "two")).unwrap();
    drop(store);

    let (mut reopened, replayed) = open_store(&path).unwrap();
    assert_eq!(replayed, vec![entry(1, "one"), entry(2, "two")]);
    assert_eq!(reopened.next_sequence(), 3);
    reopened.append(&entry(3, "three")).unwrap();
}

#[test]
fn new_store_and_parent_are_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit/events.jsonl");
    let (_store, _) = open_store(&path).unwrap();
    let directory_mode = std::fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    let file_mode = std::fs::metadata(&path).unwrap().permissions().mode();
    let key_mode = std::fs::metadata(sidecar_path(&path, "key").unwrap())
        .unwrap()
        .permissions()
        .mode();
    let checkpoint_mode = std::fs::metadata(sidecar_path(&path, "checkpoint").unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(directory_mode & 0o077, 0);
    assert_eq!(file_mode & 0o077, 0);
    assert_eq!(key_mode & 0o077, 0);
    assert_eq!(checkpoint_mode & 0o077, 0);
}

#[test]
fn tampering_and_incomplete_records_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit/events.jsonl");
    let (mut store, _) = open_store(&path).unwrap();
    store.append(&entry(1, "one")).unwrap();
    drop(store);

    let mut bytes = std::fs::read(&path).unwrap();
    let byte = bytes.iter_mut().find(|byte| **byte == b'o').unwrap();
    *byte = b'x';
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        open_store(&path),
        Err(AuditError::Hash(1)) | Err(AuditError::Decode(_)) | Err(AuditError::PreviousHash(1))
    ));

    std::fs::write(&path, b"{\"version\":1").unwrap();
    assert!(matches!(
        open_store(&path),
        Err(AuditError::IncompleteRecord(1))
    ));
}

#[test]
fn a_second_live_opener_fails_fast_instead_of_corrupting_the_chain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit/events.jsonl");
    let (mut store, _) = open_store(&path).unwrap();
    store.append(&entry(1, "one")).unwrap();
    assert!(matches!(open_store(&path), Err(AuditError::Locked(_))));
    drop(store);

    let (reopened, replayed) = open_store(&path).unwrap();
    assert_eq!(replayed, vec![entry(1, "one")]);
    assert_eq!(reopened.next_sequence(), 2);
}

#[test]
fn authenticated_checkpoint_bounds_synchronous_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit/events.jsonl");
    let options = AuditStoreOptions {
        checkpoint_interval_events: 1,
        min_free_bytes: 0,
        ..AuditStoreOptions::default()
    };
    let (mut store, _) = ChainedEventStore::open_with_options(&path, 2, options).unwrap();
    for sequence in 1..=5 {
        store
            .append(&entry(sequence, &format!("event-{sequence}")))
            .unwrap();
    }
    drop(store);

    let (mut reopened, replayed) = ChainedEventStore::open_with_options(&path, 2, options).unwrap();
    assert!(reopened.checkpoint_accelerated());
    assert_eq!(replayed, vec![entry(4, "event-4"), entry(5, "event-5")]);
    assert_eq!(reopened.next_sequence(), 6);
    reopened.append(&entry(6, "event-6")).unwrap();
}

#[test]
fn audit_log_restores_a_tail_whose_first_sequence_is_after_genesis() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit/events.jsonl");
    let options = AuditStoreOptions {
        checkpoint_interval_events: 1,
        min_free_bytes: 0,
        ..AuditStoreOptions::default()
    };
    let mut log = AuditLog::<Entry>::open_persistent_with_options(2, &path, options).unwrap();
    for sequence in 1..=5 {
        log.push_verified(entry(sequence, &format!("event-{sequence}")))
            .unwrap();
    }
    drop(log);

    let mut reopened = AuditLog::<Entry>::open_persistent_with_options(2, &path, options).unwrap();
    assert_eq!(reopened.oldest_seq(), 4);
    assert_eq!(reopened.latest_seq(), 5);
    assert_eq!(
        reopened.since(0).entries,
        vec![entry(4, "event-4"), entry(5, "event-5")]
    );
    reopened.push_verified(entry(6, "event-6")).unwrap();
}

#[test]
fn checkpoint_mac_tampering_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit/events.jsonl");
    let (mut store, _) = open_store(&path).unwrap();
    store.append(&entry(1, "one")).unwrap();
    drop(store);

    let checkpoint_path = sidecar_path(&path, "checkpoint").unwrap();
    let mut checkpoint: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&checkpoint_path).unwrap()).unwrap();
    checkpoint["mac"] = serde_json::Value::String("00".repeat(32));
    std::fs::write(&checkpoint_path, serde_json::to_vec(&checkpoint).unwrap()).unwrap();
    assert!(matches!(
        open_store(&path),
        Err(AuditError::CheckpointAuthentication)
    ));
}

#[test]
fn old_prefix_is_fully_verified_before_the_chain_can_advance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit/events.jsonl");
    let options = AuditStoreOptions {
        checkpoint_interval_events: 1,
        min_free_bytes: 0,
        ..AuditStoreOptions::default()
    };
    let (mut store, _) = ChainedEventStore::open_with_options(&path, 2, options).unwrap();
    for sequence in 1..=5 {
        store
            .append(&entry(sequence, &format!("event-{sequence}")))
            .unwrap();
    }
    drop(store);

    let mut bytes = std::fs::read(&path).unwrap();
    let first = bytes
        .windows(b"event-1".len())
        .position(|window| window == b"event-1")
        .unwrap();
    bytes[first] = b'X';
    std::fs::write(&path, bytes).unwrap();

    let (mut reopened, replayed) = ChainedEventStore::open_with_options(&path, 2, options).unwrap();
    assert_eq!(
        replayed.len(),
        2,
        "the authenticated tail restores immediately"
    );
    assert!(matches!(
        reopened.append(&entry(6, "event-6")),
        Err(AuditError::BackgroundVerification(_))
    ));
}

#[test]
fn quota_and_filesystem_reserve_refuse_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let quota_path = dir.path().join("quota/events.jsonl");
    let quota_options = AuditStoreOptions {
        max_store_bytes: (MAX_EVENT_BYTES + 1) as u64,
        min_free_bytes: 0,
        checkpoint_interval_bytes: u64::MAX,
        checkpoint_interval_events: u64::MAX,
        segment_max_bytes: (MAX_EVENT_BYTES + 1) as u64,
        retain_segments: 0,
    };
    let (mut quota_store, _) =
        ChainedEventStore::open_with_options(&quota_path, 2, quota_options).unwrap();
    let payload = "x".repeat(3 * 1024 * 1024);
    quota_store.append(&entry(1, &payload)).unwrap();
    let before = std::fs::metadata(&quota_path).unwrap().len();
    assert!(matches!(
        quota_store.append(&entry(2, &payload)),
        Err(AuditError::QuotaExceeded { .. })
    ));
    assert_eq!(std::fs::metadata(&quota_path).unwrap().len(), before);

    let reserve_path = dir.path().join("reserve/events.jsonl");
    let reserve_options = AuditStoreOptions {
        min_free_bytes: u64::MAX,
        ..AuditStoreOptions::default()
    };
    let (mut reserve_store, _) =
        ChainedEventStore::open_with_options(&reserve_path, 2, reserve_options).unwrap();
    assert!(matches!(
        reserve_store.append(&entry(1, "one")),
        Err(AuditError::LowSpace { .. })
    ));
    assert_eq!(std::fs::metadata(&reserve_path).unwrap().len(), 0);
}

#[test]
fn unsafe_permissions_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let audit_dir = dir.path().join("audit");
    std::fs::create_dir(&audit_dir).unwrap();
    let path = audit_dir.join("events.jsonl");
    std::fs::write(&path, []).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(open_store(path), Err(AuditError::UnsafePath(_))));
}

fn segment_options(segment_max_bytes: u64) -> AuditStoreOptions {
    AuditStoreOptions {
        min_free_bytes: 0,
        checkpoint_interval_events: u64::MAX,
        checkpoint_interval_bytes: u64::MAX,
        segment_max_bytes,
        retain_segments: 0,
        ..AuditStoreOptions::default()
    }
}

fn append_many(store: &mut ChainedEventStore<Entry>, from: u64, to: u64) -> Result<(), AuditError> {
    for sequence in from..to {
        store.append(&entry(sequence, &format!("event-{sequence}")))?;
    }
    Ok(())
}

#[test]
fn sealing_continues_the_chain_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit/events.jsonl");
    // Tiny segment ceiling so the first few events seal immediately.
    let options = segment_options(1);
    let (mut store, _) = ChainedEventStore::open_with_options(&path, 8, options).unwrap();
    append_many(&mut store, 1, 7).unwrap();
    // Sealing must have happened: the active stream restarts small while
    // sequence numbers continue.
    assert!(store.next_sequence() >= 7);
    let sealed_at_close = store.audit_status().sealed_segments;
    assert!(sealed_at_close >= 1, "sealing did not trigger");
    drop(store);

    let (mut reopened, replayed) =
        ChainedEventStore::<Entry>::open_with_options(&path, 8, options).unwrap();
    assert_eq!(reopened.next_sequence(), 7);
    assert!(
        replayed.iter().all(|entry: &Entry| entry.seq <= 6),
        "replayed tail must come from the current epoch"
    );
    // The chain continues across the seal without a sequence reset.
    append_many(&mut reopened, 7, 10).unwrap();
    assert_eq!(reopened.next_sequence(), 10);
    drop(reopened);

    let (final_store, _) =
        ChainedEventStore::<Entry>::open_with_options(&path, 8, options).unwrap();
    let status = final_store.audit_status();
    assert_eq!(status.next_sequence, 10);
    assert!(status.sealed_segments >= sealed_at_close);
}

#[test]
fn fast_verification_detects_segment_tampering() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit/events.jsonl");
    let options = segment_options(1);
    let (mut store, _) = ChainedEventStore::open_with_options(&path, 8, options).unwrap();
    append_many(&mut store, 1, 6).unwrap();
    drop(store);

    // Corrupt one sealed segment byte.
    let segments_dir = path.with_file_name(segments::SEGMENTS_DIR);
    let segment = std::fs::read_dir(&segments_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut bytes = std::fs::read(&segment).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&segment, bytes).unwrap();

    let (store, _) = ChainedEventStore::<Entry>::open_with_options(&path, 8, options).unwrap();
    let status = store.audit_status();
    assert!(status.sealed_segments >= 1);
    // The manifest-level fast check reports the corruption.
    let manifest = segments::SegmentManifest::open(
        &path,
        &std::fs::read(path.with_file_name("events.jsonl.key")).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        manifest.verify_fast(),
        Err(AuditError::SegmentVerification { .. })
    ));
}

#[test]
fn retention_prunes_only_exported_segments_and_records_removal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit/events.jsonl");
    let mut options = segment_options(1);
    options.retain_segments = 2;
    let (mut store, _) = ChainedEventStore::open_with_options(&path, 8, options).unwrap();
    append_many(&mut store, 1, 12).unwrap();
    drop(store);

    let key = std::fs::read(path.with_file_name("events.jsonl.key")).unwrap();
    let mut manifest = segments::SegmentManifest::open(&path, &key).unwrap();
    let sealed = manifest.segments().len();
    assert!(sealed > 2, "expected multiple sealed segments");

    // Pruning without export acknowledgements refuses.
    assert!(manifest.prune(2, true).is_err());

    // After acknowledging an export, pruning removes all but two.
    manifest.mark_exported("/mnt/audit-archive").unwrap();
    let removed = manifest.prune(2, true).unwrap();
    assert_eq!(removed.len(), sealed - 2);
    assert_eq!(manifest.segments().len(), 2);
    assert_eq!(manifest.body().pruned.len(), removed.len());

    // Fast verification still passes for the retained segments.
    assert_eq!(manifest.verify_fast().unwrap(), 2);

    // The manifest itself authenticates: tampering is detected.
    let manifest_file = segments::manifest_path(&path);
    let mut body = std::fs::read(&manifest_file).unwrap();
    body[10] ^= 0x01;
    std::fs::write(&manifest_file, body).unwrap();
    assert!(matches!(
        segments::SegmentManifest::open(&path, &key),
        Err(AuditError::CheckpointAuthentication) | Err(AuditError::CheckpointDecode(_))
    ));
}

#[test]
fn sealing_preserves_chain_integrity_across_decompression() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit/events.jsonl");
    let options = segment_options(1);
    let (mut store, _) = ChainedEventStore::open_with_options(&path, 8, options).unwrap();
    append_many(&mut store, 1, 8).unwrap();
    drop(store);

    let key = std::fs::read(path.with_file_name("events.jsonl.key")).unwrap();
    let manifest = segments::SegmentManifest::open(&path, &key).unwrap();
    let segments_root = path.with_file_name(segments::SEGMENTS_DIR);
    // Walk the sealed chain: every segment's tail must equal the next
    // segment's previous anchor.
    let records = manifest.segments();
    for pair in records.windows(2) {
        assert_eq!(
            pair[0].tail_hash, pair[1].previous_hash,
            "sealed segments must chain"
        );
        assert_eq!(
            pair[0].last_sequence_exclusive, pair[1].first_sequence,
            "sealed sequences must be contiguous"
        );
    }
    // Decompress one segment and verify its JSONL lines parse.
    let record = &records[0];
    let mut reader = segments::SealedSegmentReader::open(record, &segments_root).unwrap();
    let mut lines = 0usize;
    reader
        .for_each_line(|line| {
            assert_eq!(line.last(), Some(&b'\n'));
            let envelope: serde_json::Value =
                serde_json::from_slice(&line[..line.len() - 1]).map_err(AuditError::Decode)?;
            assert!(envelope["sequence"].is_u64());
            lines += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        lines as u64,
        record.last_sequence_exclusive - record.first_sequence,
        "every sealed event must be present after decompression"
    );
}
