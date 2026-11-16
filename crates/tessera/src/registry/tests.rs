use super::*;

fn scratch() -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tessera-agent-auth-{}-{n}", std::process::id()));
    std::fs::create_dir(&dir).expect("create private test directory");
    dir.join("state.json")
}

fn cleanup(path: &std::path::Path) {
    let dir = path.parent().expect("test state parent");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn issue_sanitizes_splits_and_recognizes_the_credential() {
    let mut registry = PrincipalRegistry::in_memory();
    let paired = registry
        .issue(
            Some("Codex"),
            &[
                ActorCapability::Focus,
                ActorCapability::CaptureInteractionDomain,
                ActorCapability::Focus,
                ActorCapability::PickConfirm, // component-only, must be dropped
            ],
        )
        .expect("issue");
    assert_eq!(paired.pregranted, vec![ActorCapability::Focus]);
    assert_eq!(
        paired.gated,
        vec![ActorCapability::CaptureInteractionDomain]
    );

    let identity = registry.lookup(&paired.credential).expect("recognized");
    assert_eq!(identity.principal, paired.principal);
    assert_eq!(identity.pregranted, vec![ActorCapability::Focus]);
    assert_eq!(
        identity.gated,
        vec![ActorCapability::CaptureInteractionDomain]
    );
    assert!(registry.lookup("forged").is_none());
    assert_eq!(
        registry
            .identity_for_principal(&paired.principal)
            .expect("live principal")
            .gated,
        vec![ActorCapability::CaptureInteractionDomain]
    );
}

#[test]
fn registry_survives_a_save_load_cycle() {
    let path = scratch();
    let mut registry = PrincipalRegistry::load(path.clone());
    let paired = registry
        .issue(Some("Codex"), &[ActorCapability::Notify])
        .expect("issue");
    drop(registry);

    let reloaded = PrincipalRegistry::load(path.clone());
    let identity = reloaded.lookup(&paired.credential).expect("persisted");
    assert_eq!(identity.principal, paired.principal);
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);
    cleanup(&path);
}

#[test]
fn corrupt_or_version_mismatched_files_start_empty_fail_closed() {
    let path = scratch();
    std::fs::write(&path, b"not json").unwrap();
    let registry = PrincipalRegistry::load(path.clone());
    assert!(registry.lookup("anything").is_none());

    std::fs::write(&path, br#"{"version":99,"principals":[]}"#).unwrap();
    let registry = PrincipalRegistry::load(path.clone());
    assert!(registry.lookup("anything").is_none());
    cleanup(&path);
}

#[test]
fn unsafe_or_semantically_invalid_registry_files_start_empty() {
    use std::os::unix::fs::PermissionsExt as _;

    let path = scratch();
    let mut registry = PrincipalRegistry::load(path.clone());
    let paired = registry
        .issue(Some("Codex"), &[ActorCapability::Focus])
        .unwrap();
    drop(registry);

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        PrincipalRegistry::load(path.clone())
            .lookup(&paired.credential)
            .is_none(),
        "group/world-readable credential registry was trusted"
    );

    let invalid = RegistryFile {
        version: REGISTRY_VERSION,
        principals: vec![PrincipalRecord {
            id: "prin_invalid".into(),
            label: Some("Injected".into()),
            credential_sha256: "0".repeat(64),
            pregranted: vec![ActorCapability::PickConfirm],
            gated: Vec::new(),
            created_at: 1,
        }],
    };
    std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        PrincipalRegistry::load(path.clone())
            .principals()
            .is_empty()
    );
    cleanup(&path);
}

#[test]
fn labels_are_bounded_at_the_registry_boundary() {
    let mut registry = PrincipalRegistry::in_memory();
    assert!(
        registry
            .issue(Some(" "), &[ActorCapability::Focus])
            .is_err()
    );
    assert!(
        registry
            .register(
                Some(&"x".repeat(257)),
                vec![ActorCapability::Focus],
                Vec::new(),
            )
            .is_err()
    );
}

#[test]
fn label_collision_and_session_denial_are_tracked() {
    let mut registry = PrincipalRegistry::in_memory();
    registry
        .issue(Some("Codex"), &[ActorCapability::Focus])
        .unwrap();
    assert!(registry.label_collision("codex"));
    assert!(!registry.label_collision("OpenCode"));

    assert!(!registry.is_denied(Some("OpenCode")));
    registry.deny(Some("OpenCode"));
    assert!(registry.is_denied(Some("opencode")));
    assert!(!registry.is_denied(Some("Codex")));
}

#[test]
fn capability_families_filter_dedup_and_mark_gated() {
    let families = capability_families(&[
        ActorCapability::Focus,
        ActorCapability::CaptureInteractionDomain,
        ActorCapability::Focus,
        ActorCapability::PickConfirm, // component-only, must be dropped
    ]);
    assert_eq!(families.len(), 2);
    assert_eq!(families[0].key, "windows");
    let focus = &families[0].members[0];
    assert_eq!(focus.key, "Focus");
    assert_eq!(focus.label, "Focus windows");
    assert!(!focus.gated);
    assert_eq!(families[1].key, "domains");
    let capture = &families[1].members[0];
    assert_eq!(capture.key, "CaptureInteractionDomain");
    assert_eq!(capture.label, "Capture its Interaction Domain");
    assert!(capture.gated);
}

#[test]
fn capability_families_cover_every_requestable_op_exactly_once() {
    let mut table_ops: Vec<ActorCapability> = CAPABILITY_FAMILIES
        .iter()
        .flat_map(|(_, _, ops)| ops.iter().copied())
        .collect();
    table_ops.sort_by_key(|op| op_key(*op));
    let mut requestable = AGENT_REQUESTABLE.to_vec();
    requestable.sort_by_key(|op| op_key(*op));
    assert_eq!(table_ops, requestable);
}

#[test]
fn capability_family_member_keys_round_trip_through_from_name() {
    let families = capability_families(AGENT_REQUESTABLE);
    assert_eq!(families.len(), CAPABILITY_FAMILIES.len());
    for family in &families {
        assert!(!family.members.is_empty());
        for member in &family.members {
            assert_eq!(
                ActorCapability::from_name(member.key).map(|op| op.label()),
                Some(member.label)
            );
        }
    }
}

#[test]
fn rename_forget_and_set_ceiling_manage_principals() {
    let mut registry = PrincipalRegistry::in_memory();
    let paired = registry
        .issue(Some("Codex"), &[ActorCapability::Focus])
        .unwrap();

    registry.rename(&paired.principal, Some("Renamed")).unwrap();
    assert_eq!(registry.principals()[0].label.as_deref(), Some("Renamed"));
    registry.rename(&paired.principal, None).unwrap();
    assert_eq!(registry.principals()[0].label, None);
    assert!(registry.rename("prin_missing", Some("x")).is_err());

    registry
        .set_ceiling(
            &paired.principal,
            vec![ActorCapability::Focus],
            vec![ActorCapability::Close],
        )
        .unwrap();
    assert_eq!(
        registry.principals()[0].pregranted,
        vec![ActorCapability::Focus]
    );
    assert_eq!(registry.principals()[0].gated, vec![ActorCapability::Close]);
    assert!(
        registry
            .set_ceiling("prin_missing", vec![], vec![])
            .is_err()
    );

    registry.forget(&paired.principal).unwrap();
    assert!(registry.principals().is_empty());
    assert!(registry.lookup(&paired.credential).is_none());
    assert!(registry.identity_for_principal(&paired.principal).is_none());
    assert!(registry.forget(&paired.principal).is_err());
}

#[test]
fn register_validates_and_issues_a_working_credential() {
    let mut registry = PrincipalRegistry::in_memory();
    let (principal, credential) = registry
        .register(
            Some("Provisioned"),
            vec![ActorCapability::Focus],
            vec![ActorCapability::Close],
        )
        .expect("register");
    let identity = registry.lookup(&credential).expect("recognized");
    assert_eq!(identity.principal.as_str(), principal);
    assert_eq!(identity.pregranted, vec![ActorCapability::Focus]);
    assert_eq!(identity.gated, vec![ActorCapability::Close]);
    assert_eq!(
        registry.principals()[0].label.as_deref(),
        Some("Provisioned")
    );
}

#[test]
fn explicit_ceilings_reject_unsafe_or_ambiguous_assignments() {
    let mut registry = PrincipalRegistry::in_memory();
    let paired = registry
        .issue(Some("Codex"), &[ActorCapability::Focus])
        .unwrap();

    assert!(
        registry
            .set_ceiling(&paired.principal, vec![ActorCapability::Close], vec![])
            .unwrap_err()
            .contains("must be runtime-gated")
    );
    assert!(
        registry
            .set_ceiling(
                &paired.principal,
                vec![ActorCapability::PickConfirm],
                vec![],
            )
            .unwrap_err()
            .contains("component-only")
    );
    assert!(
        registry
            .set_ceiling(
                &paired.principal,
                vec![ActorCapability::Focus],
                vec![ActorCapability::Focus],
            )
            .unwrap_err()
            .contains("both pregranted")
    );
    assert_eq!(
        registry.principals()[0].pregranted,
        vec![ActorCapability::Focus]
    );
}

#[test]
fn grant_session_decisions_win_over_durable_ones() {
    let mut store = GrantStore::in_memory();
    assert_eq!(store.decision_for("prin_a", ActorCapability::Close), None);

    store
        .record("prin_a", ActorCapability::Close, true, false)
        .unwrap();
    assert_eq!(
        store.decision_for("prin_a", ActorCapability::Close),
        Some(true)
    );

    store
        .record("prin_a", ActorCapability::Close, false, true)
        .unwrap();
    assert_eq!(
        store.decision_for("prin_a", ActorCapability::Close),
        Some(false)
    );

    // A durable record supersedes the session decision.
    store
        .record("prin_a", ActorCapability::Close, true, false)
        .unwrap();
    assert_eq!(
        store.decision_for("prin_a", ActorCapability::Close),
        Some(true)
    );
}

#[test]
fn grant_store_survives_a_save_load_cycle() {
    let path = scratch();
    let mut store = GrantStore::load(path.clone());
    store
        .record("prin_a", ActorCapability::Close, true, false)
        .unwrap();
    store
        .record("prin_b", ActorCapability::Notify, false, false)
        .unwrap();
    store
        .record("prin_a", ActorCapability::Focus, true, true)
        .unwrap();
    drop(store);

    let reloaded = GrantStore::load(path.clone());
    assert_eq!(
        reloaded.decision_for("prin_a", ActorCapability::Close),
        Some(true)
    );
    // Session decisions are not persisted.
    assert_eq!(
        reloaded.decision_for("prin_a", ActorCapability::Focus),
        None
    );
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);

    let listed = reloaded.list(Some("prin_a"));
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].op, ActorCapability::Close);
    assert_eq!(
        listed[0].decision,
        tessera_protocol::AgentGrantDecision::Allow
    );
    assert!(listed[0].granted_at > 0);
    let all = reloaded.list(None);
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].decision, tessera_protocol::AgentGrantDecision::Deny);
    cleanup(&path);
}

#[test]
fn grant_revoke_and_forget_principal_clear_decisions() {
    let mut store = GrantStore::in_memory();
    store
        .record("prin_a", ActorCapability::Close, true, false)
        .unwrap();
    store
        .record("prin_a", ActorCapability::Focus, true, true)
        .unwrap();
    store
        .record("prin_b", ActorCapability::Close, false, false)
        .unwrap();

    store.revoke("prin_a", ActorCapability::Close).unwrap();
    assert_eq!(store.decision_for("prin_a", ActorCapability::Close), None);
    assert_eq!(
        store.decision_for("prin_a", ActorCapability::Focus),
        Some(true)
    );
    assert_eq!(
        store.decision_for("prin_b", ActorCapability::Close),
        Some(false)
    );

    store.forget_principal("prin_a");
    assert_eq!(store.decision_for("prin_a", ActorCapability::Focus), None);
    assert_eq!(
        store.decision_for("prin_b", ActorCapability::Close),
        Some(false)
    );
}

#[test]
fn corrupt_or_version_mismatched_grant_files_start_empty_fail_closed() {
    let path = scratch();
    std::fs::write(&path, b"not json").unwrap();
    let store = GrantStore::load(path.clone());
    assert_eq!(store.decision_for("prin_a", ActorCapability::Close), None);

    std::fs::write(&path, br#"{"version":99,"grants":[]}"#).unwrap();
    let store = GrantStore::load(path.clone());
    assert_eq!(store.decision_for("prin_a", ActorCapability::Close), None);
    cleanup(&path);
}
