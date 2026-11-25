//! End-to-end coverage of the protocol-28 primitive families (ADR-0125):
//! Transact preflight/commit/conflict, Observe's per-class scope gating,
//! and scope-filtered agent subscription lanes.

use super::*;

fn paired_handler(pregranted: Vec<ActorCapability>) -> (Arc<TestHandler>, PathBuf) {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    *handler.pair_result.lock().unwrap() = Ok(tessera_authority::authority::PairedAgent {
        principal: tessera_authority::authority::ActorPrincipal::new("prin_prim").unwrap(),
        credential: "cred_prim".into(),
        pregranted,
        gated: vec![],
    });
    (handler, path)
}

fn paired_client(path: &std::path::Path, caps: ConnectionCapabilities) -> Client {
    Client::connect_agent_with_timeout(
        path,
        caps,
        None,
        tessera_protocol::AgentHello {
            label: Some("primitives".into()),
            requested: vec![],
            credential: None,
        },
        Duration::from_secs(5),
    )
    .expect("pairing connects")
}

#[test]
fn transact_commits_and_returns_per_op_receipts() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    )
    .expect("connect");

    let result = client
        .transact(
            None,
            None,
            vec![
                tessera_protocol::TransactOp::Focus {
                    id: WindowId(1),
                    reveal: false,
                },
                tessera_protocol::TransactOp::Minimize { id: WindowId(2) },
            ],
        )
        .expect("transact");
    let tessera_protocol::TransactResult::Committed { receipt } = result else {
        panic!("expected commit, got {result:?}");
    };
    assert_eq!(receipt.before_seq, 0);
    assert_eq!(receipt.after_seq, 2);
    assert_eq!(
        receipt
            .results
            .iter()
            .map(|result| (result.seq, &result.effect))
            .collect::<Vec<_>>(),
        vec![
            (1, &tessera_protocol::Effect::Applied),
            (2, &tessera_protocol::Effect::Applied)
        ]
    );
    assert_eq!(
        handler.commands.lock().unwrap().as_slice(),
        [
            Command::Focus {
                id: WindowId(1),
                reveal: false
            },
            Command::Minimize { id: WindowId(2) }
        ]
    );
}

#[test]
fn transact_preflight_refuses_out_of_scope_batch_without_applying() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_scoped_with_timeout(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
        "focus-first",
        Duration::from_secs(5),
    )
    .expect("connect");

    let error = client
        .transact(
            None,
            None,
            vec![
                tessera_protocol::TransactOp::Focus {
                    id: WindowId(1),
                    reveal: true,
                },
                tessera_protocol::TransactOp::Close { id: WindowId(1) },
            ],
        )
        .expect_err("an out-of-scope op refuses the whole batch");
    assert!(error.to_string().contains("out of scope"), "{error}");
    assert!(
        handler.commands.lock().unwrap().is_empty(),
        "no op may apply when the batch refuses"
    );
    let refusals = handler.refusals.lock().unwrap();
    assert_eq!(refusals.len(), 1);
    assert!(
        matches!(
            &refusals[0].1,
            tessera_protocol::JournalMutation::Command {
                cmd: tessera_protocol::AuditedCommand::Close { .. }
            }
        ),
        "the refused op is the audited one: {:?}",
        refusals[0].1
    );
}

#[test]
fn transact_precondition_conflict_applies_nothing() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    )
    .expect("connect");

    let result = client
        .transact(
            None,
            None,
            vec![tessera_protocol::TransactOp::SwitchWorkspace {
                dir: tessera_desktop::workspace::Switch::Next,
            }],
        )
        .expect("first transact");
    let tessera_protocol::TransactResult::Committed { receipt } = result else {
        panic!("expected commit, got {result:?}");
    };
    assert_eq!(receipt.after_seq, 1);

    let result = client
        .transact(
            Some(receipt.before_seq),
            None,
            vec![tessera_protocol::TransactOp::Minimize { id: WindowId(1) }],
        )
        .expect("conflicting transact");
    assert_eq!(
        result,
        tessera_protocol::TransactResult::PreconditionConflict {
            precondition: tessera_protocol::TransactPrecondition::JournalSeq,
            expected: 0,
            actual: 1
        }
    );
    assert_eq!(
        handler.commands.lock().unwrap().len(),
        1,
        "a conflicting batch applies nothing"
    );

    let result = client
        .transact(
            Some(receipt.after_seq),
            None,
            vec![tessera_protocol::TransactOp::Minimize { id: WindowId(1) }],
        )
        .expect("retry with the fresh cursor");
    assert!(
        matches!(result, tessera_protocol::TransactResult::Committed { .. }),
        "a retry at the fresh cursor commits: {result:?}"
    );
}

#[test]
fn transact_rejects_empty_and_invalid_batches() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    )
    .expect("connect");

    let error = client
        .transact(None, None, vec![])
        .expect_err("an empty batch is refused");
    assert!(error.to_string().contains("ops"), "{error}");
    let error = client
        .transact(
            None,
            None,
            vec![tessera_protocol::TransactOp::SetWindowGeometry {
                id: WindowId(1),
                rect: tessera_primitives::Rect::new(0, 0, 0, 10),
            }],
        )
        .expect_err("an invalid op is refused");
    assert!(!error.to_string().is_empty());
    assert!(handler.commands.lock().unwrap().is_empty());
}

#[test]
fn observe_returns_all_classes_for_anonymous_owner_clients() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect(&path).expect("connect");

    let snapshot = client.observe().expect("observe");
    assert_eq!(snapshot.windows.expect("windows").len(), 2);
    assert!(snapshot.workspaces.is_some());
    assert!(snapshot.outputs.is_some());
    assert!(snapshot.notifications.is_some());
    assert!(snapshot.interaction_domains.is_some());
    let cursor = snapshot.journal_cursor.expect("journal cursor");
    assert_eq!(cursor.latest_seq, 0);
}

#[test]
fn observe_gates_each_class_by_agent_scope() {
    let (handler, path) = paired_handler(vec![ActorCapability::ObserveWindows]);
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = paired_client(&path, ConnectionCapabilities::QUERY);

    let snapshot = client.observe().expect("observe");
    assert_eq!(snapshot.windows.expect("windows").len(), 2);
    assert!(snapshot.workspaces.is_none());
    assert!(snapshot.outputs.is_none());
    assert!(snapshot.notifications.is_none());
    assert!(snapshot.interaction_domains.is_none());
    assert!(snapshot.journal_cursor.is_none());
}

#[test]
fn agent_coarse_subscription_is_gated_by_observation_scope() {
    let (handler, path) = paired_handler(vec![ActorCapability::ObserveWindows]);
    let server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = paired_client(&path, ConnectionCapabilities::QUERY);
    client.subscribe().expect("subscribe");

    server.broadcast(Event::WorkspaceChanged);
    server.broadcast(Event::InteractionDomainsChanged { revision: 9 });
    server.broadcast(Event::WindowsChanged);

    client
        .set_io_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    match client.next_event().expect("one permitted event") {
        Event::WindowsChanged => {}
        other => panic!("expected WindowsChanged, got {other:?}"),
    }
    let error = client
        .next_event()
        .expect_err("scope-refused events are not delivered");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock, "{error}");
}

#[test]
fn agent_journal_subscription_filters_by_subject_and_scope() {
    let (handler, path) = paired_handler(vec![
        ActorCapability::ObserveJournal,
        ActorCapability::Focus,
    ]);
    let server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = paired_client(&path, ConnectionCapabilities::QUERY);
    client.subscribe_journal().expect("subscribe journal");

    let focus = tessera_protocol::JournalMutation::Command {
        cmd: tessera_protocol::AuditedCommand::from(&Command::Focus {
            id: WindowId(1),
            reveal: true,
        }),
    };
    let close = tessera_protocol::JournalMutation::Command {
        cmd: tessera_protocol::AuditedCommand::from(&Command::Close { id: WindowId(1) }),
    };
    // Another principal's mutation is never visible to this agent.
    server.broadcast_journal(std::sync::Arc::new(tessera_protocol::JournalEntry {
        seq: 1,
        ts_mono_ms: 1,
        ts_wall_ms: 1_700_000_000_001,
        topic: tessera_protocol::EventTopic::Window,
        correlation_id: None,
        origin: tessera_protocol::Origin::Actor {
            conn_id: 99,
            principal: "prin_other".into(),
        },
        mutation: focus.clone(),
        effect: tessera_protocol::Effect::Applied,
    }));
    // A resource-permitted entry from a non-Actor origin passes through,
    // matching `GetJournal`'s filter exactly.
    server.broadcast_journal(std::sync::Arc::new(tessera_protocol::JournalEntry {
        seq: 2,
        ts_mono_ms: 2,
        ts_wall_ms: 1_700_000_000_002,
        topic: tessera_protocol::EventTopic::Window,
        correlation_id: None,
        origin: tessera_protocol::Origin::Ipc { conn_id: 3 },
        mutation: close,
        effect: tessera_protocol::Effect::Applied,
    }));
    // The agent's own permitted mutation is delivered.
    server.broadcast_journal(std::sync::Arc::new(tessera_protocol::JournalEntry {
        seq: 3,
        ts_mono_ms: 3,
        ts_wall_ms: 1_700_000_000_003,
        topic: tessera_protocol::EventTopic::Window,
        correlation_id: None,
        origin: tessera_protocol::Origin::Actor {
            conn_id: 1,
            principal: "prin_prim".into(),
        },
        mutation: focus,
        effect: tessera_protocol::Effect::Applied,
    }));

    client
        .set_io_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    // The other principal's entry was filtered; the resource-permitted
    // entries arrive in order, matching `GetJournal`'s filter exactly.
    for expected_seq in [2, 3] {
        match client.next_event().expect("permitted entry") {
            Event::Journal { entry } => assert_eq!(entry.seq, expected_seq),
            other => panic!("expected a journal event, got {other:?}"),
        }
    }
    let error = client
        .next_event()
        .expect_err("filtered entries are not delivered");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock, "{error}");
}

#[test]
fn agent_subscriptions_observe_scope_narrowing_live() {
    let (handler, path) = paired_handler(vec![ActorCapability::ObserveWindows]);
    let server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = paired_client(&path, ConnectionCapabilities::QUERY);
    client.subscribe().expect("subscribe");

    server.broadcast(Event::WindowsChanged);
    client
        .set_io_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    assert!(matches!(
        client.next_event().expect("event while permitted"),
        Event::WindowsChanged
    ));

    // The principal registry narrowing mid-stream (a forgotten agent fails
    // closed) stops delivery without dropping the lane.
    *handler.refresh_result.lock().unwrap() = Err("forgotten principal".into());
    server.broadcast(Event::WindowsChanged);
    let error = client
        .next_event()
        .expect_err("a forgotten principal stops delivery");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock, "{error}");
}

#[test]
fn transact_interaction_domain_revision_precondition() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    )
    .expect("connect");

    // The test handler's Interaction Domain snapshot carries revision 4.
    let result = client
        .transact(
            None,
            Some(3),
            vec![tessera_protocol::TransactOp::SwitchWorkspace {
                dir: tessera_desktop::workspace::Switch::Next,
            }],
        )
        .expect("conflicting transact");
    assert_eq!(
        result,
        tessera_protocol::TransactResult::PreconditionConflict {
            precondition: tessera_protocol::TransactPrecondition::InteractionDomainRevision,
            expected: 3,
            actual: 4,
        }
    );
    assert!(handler.commands.lock().unwrap().is_empty());

    let result = client
        .transact(
            None,
            Some(4),
            vec![tessera_protocol::TransactOp::SwitchWorkspace {
                dir: tessera_desktop::workspace::Switch::Next,
            }],
        )
        .expect("transact at the observed revision");
    assert!(
        matches!(result, tessera_protocol::TransactResult::Committed { .. }),
        "the batch commits when the revision holds: {result:?}"
    );
}

#[test]
fn connection_state_echoes_the_live_grant() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    )
    .expect("connect");
    let (caps, scope, lease, session) = client.connection_state().expect("connection state");
    assert!(caps.query && caps.control && !caps.input);
    assert_eq!(scope, client.scope().clone());
    assert!(lease.is_some(), "a privileged connection holds a lease");
    assert!(session.is_some());

    // A paired agent reads its registry ceiling, re-resolved live.
    let (handler, path) = paired_handler(vec![ActorCapability::Focus]);
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut agent = paired_client(&path, ConnectionCapabilities::QUERY);
    let (_, scope, _, _) = agent.connection_state().expect("agent connection state");
    assert_eq!(scope.ops, Some(vec![ActorCapability::Focus]));

    *handler.refresh_result.lock().unwrap() =
        Ok(Some(tessera_authority::authority::AgentIdentity {
            principal: tessera_authority::authority::ActorPrincipal::new("prin_prim").unwrap(),
            pregranted: vec![ActorCapability::Focus, ActorCapability::Close],
            gated: vec![],
        }));
    let (_, scope, _, _) = agent.connection_state().expect("re-resolved state");
    assert_eq!(
        scope.ops,
        Some(vec![ActorCapability::Focus, ActorCapability::Close]),
        "ceiling changes are visible without reconnecting"
    );
}

#[test]
fn unified_event_bus_supports_topic_filtered_subscription_and_publish_event() {
    struct BusHandler {
        journal: std::sync::Mutex<tessera_audit::AuditLog<tessera_audit::journal::Entry>>,
        broadcaster: tessera_ipc::JournalBroadcaster,
    }

    impl Handler for BusHandler {
        fn policy_caps(&self) -> ConnectionCapabilities {
            ConnectionCapabilities {
                query: true,
                control: true,
                input: false,
                session: false,
                interaction_domain: false,
            }
        }
        fn windows(&self) -> Vec<tessera_desktop::window::Window> {
            Vec::new()
        }
        fn workspaces(&self) -> tessera_desktop::workspace::WorkspaceSnapshot {
            tessera_desktop::workspace::WorkspaceSnapshot {
                outputs: Vec::new(),
            }
        }
        fn command(&self, _conn_id: u64, _principal: Option<&str>, _cmd: Command) {}
        fn notifications(&self) -> Vec<tessera_desktop::notify::Notification> {
            Vec::new()
        }
        fn outputs(&self) -> Vec<tessera_desktop::output::OutputInfo> {
            Vec::new()
        }
        fn journal_since(&self, since: u64) -> tessera_protocol::JournalSnapshot {
            self.journal.lock().unwrap().since(since).into()
        }
        fn publish_event(
            &self,
            conn_id: u64,
            principal: Option<&str>,
            topic: tessera_protocol::EventTopic,
            source: String,
            kind: String,
            correlation_id: Option<u64>,
            detail: Option<serde_json::Value>,
        ) -> Result<u64, String> {
            let entry = self.journal.lock().unwrap().append_correlated(
                100,
                tessera_audit::now_wall_ms(),
                correlation_id,
                tessera_protocol::Origin::ipc(conn_id, principal),
                tessera_protocol::JournalMutation::External {
                    topic,
                    source,
                    kind,
                    detail,
                },
                tessera_protocol::Effect::Applied,
            );
            let seq = entry.seq;
            self.broadcaster.broadcast(Arc::new(entry.into()));
            Ok(seq)
        }
    }

    let path = scratch();
    let broadcaster = tessera_ipc::JournalBroadcaster::default();
    let handler = Arc::new(BusHandler {
        journal: std::sync::Mutex::new(tessera_audit::AuditLog::new(16)),
        broadcaster: broadcaster.clone(),
    });
    let _server = Server::start_with_journal_broadcaster(&path, Arc::clone(&handler), broadcaster)
        .expect("bind");

    let mut mcp_sub = Client::connect_with(&path, ConnectionCapabilities::QUERY).expect("sub");
    mcp_sub
        .subscribe_journal_topics(Some(vec![tessera_protocol::EventTopic::Mcp]))
        .expect("subscribe mcp topic");
    mcp_sub
        .set_io_timeout(Some(Duration::from_millis(500)))
        .unwrap();

    let mut publisher = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    )
    .expect("publisher");

    // A non-MCP event must not reach the MCP-filtered subscriber.
    let seq1 = publisher
        .publish_event(
            tessera_protocol::EventTopic::System,
            "tessera-idle",
            "display.dimmed",
            Some(10),
            None,
        )
        .expect("publish system event");
    assert_eq!(seq1, 1);

    // An MCP event with correlation_id and detail must reach the subscriber.
    let seq2 = publisher
        .publish_event(
            tessera_protocol::EventTopic::Mcp,
            "tessera-mcp",
            "tool.called",
            Some(42),
            Some(serde_json::json!({"tool": "capture_window"})),
        )
        .expect("publish mcp event");
    assert_eq!(seq2, 2);

    match mcp_sub.next_event().expect("mcp event delivered") {
        Event::Journal { entry } => {
            assert_eq!(entry.seq, 2);
            assert_eq!(entry.topic, tessera_protocol::EventTopic::Mcp);
            assert_eq!(entry.correlation_id, Some(42));
            match entry.mutation {
                tessera_protocol::JournalMutation::External {
                    topic,
                    source,
                    kind,
                    detail,
                } => {
                    assert_eq!(topic, tessera_protocol::EventTopic::Mcp);
                    assert_eq!(source, "tessera-mcp");
                    assert_eq!(kind, "tool.called");
                    assert_eq!(detail.unwrap()["tool"], "capture_window");
                }
                other => panic!("expected External mutation, got {other:?}"),
            }
        }
        other => panic!("expected Journal event, got {other:?}"),
    }

    // Topic-filtered GetJournal returns only matching entries while preserving global bounds.
    let filtered = publisher
        .journal_filtered(0, Some(vec![tessera_protocol::EventTopic::Mcp]))
        .expect("filtered snapshot");
    assert_eq!(filtered.oldest_seq, 1);
    assert_eq!(filtered.latest_seq, 2);
    assert_eq!(filtered.entries.len(), 1);
    assert_eq!(filtered.entries[0].seq, 2);

    // Dynamic topic filter update on an already-subscribed lane works in-place.
    mcp_sub
        .subscribe_journal_topics(Some(vec![tessera_protocol::EventTopic::System]))
        .expect("update topic filter");
    let seq3 = publisher
        .publish_event(
            tessera_protocol::EventTopic::System,
            "tessera-idle",
            "display.resumed",
            None,
            None,
        )
        .expect("publish system resumed");
    assert_eq!(seq3, 3);
    match mcp_sub.next_event().expect("updated topic delivered") {
        Event::Journal { entry } => {
            assert_eq!(entry.seq, 3);
            assert_eq!(entry.topic, tessera_protocol::EventTopic::System);
        }
        other => panic!("expected Journal event, got {other:?}"),
    }

    // Security & bounds enforcement on PublishEvent:
    // 1. Query-only client without `control` is refused.
    let mut query_only =
        Client::connect_with(&path, ConnectionCapabilities::QUERY).expect("query client");
    assert!(
        query_only
            .publish_event(
                tessera_protocol::EventTopic::Mcp,
                "tessera-mcp",
                "tool.called",
                None,
                None,
            )
            .is_err()
    );
    // 2. Empty source/kind is refused.
    assert!(
        publisher
            .publish_event(
                tessera_protocol::EventTopic::Mcp,
                "",
                "tool.called",
                None,
                None
            )
            .is_err()
    );
    // 3. Oversized detail (> 16 KiB) is refused.
    let huge = serde_json::json!({"blob": "x".repeat(20 * 1024)});
    assert!(
        publisher
            .publish_event(
                tessera_protocol::EventTopic::Mcp,
                "tessera-mcp",
                "tool.called",
                None,
                Some(huge),
            )
            .is_err()
    );
}
