//! Manual end-to-end smoke test for the SNI tray, against a real session
//! bus. Run it on a private bus so it cannot clash with a running desktop's
//! own StatusNotifierWatcher:
//!
//! ```sh
//! dbus-run-session -- cargo nextest run -p tessera-tests --test e2e \
//!   -E 'test(tray_smoke)' --run-ignored only --no-capture
//! ```
//!
//! Exercises the full loop: watcher name ownership, item registration,
//! property mirroring into the snapshot, pixmap conversion, and `Activate`
//! delivery back to the item. Not run by default (needs a session bus with
//! a free `org.kde.StatusNotifierWatcher` name).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tessera_dbus::{Connection, Message, ObjectHandler, OwnedObjectPath, Proxy, Value};
use tessera_shell::tray::{TrayCommand, TrayIcon};

/// A minimal fake StatusNotifierItem: fixed properties, and a record of the
/// activation methods the watcher invoked.
struct FakeItem {
    calls: Arc<Mutex<Vec<String>>>,
    title: Arc<Mutex<String>>,
}

impl ObjectHandler for FakeItem {
    fn handle_method_call(
        &self,
        _conn: &Connection,
        msg: &Message,
    ) -> Result<Vec<Value>, (String, String)> {
        match (msg.interface.as_deref(), msg.member.as_deref()) {
            (Some("org.kde.StatusNotifierItem"), Some("Activate")) => {
                let x = msg.body.first().and_then(Value::as_i32).unwrap_or(0);
                let y = msg.body.get(1).and_then(Value::as_i32).unwrap_or(0);
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("Activate({x},{y})"));
                Ok(Vec::new())
            }
            (Some("org.kde.StatusNotifierItem"), Some("SecondaryActivate")) => {
                let x = msg.body.first().and_then(Value::as_i32).unwrap_or(0);
                let y = msg.body.get(1).and_then(Value::as_i32).unwrap_or(0);
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("SecondaryActivate({x},{y})"));
                Ok(Vec::new())
            }
            (Some("org.kde.StatusNotifierItem"), Some("ContextMenu")) => {
                let x = msg.body.first().and_then(Value::as_i32).unwrap_or(0);
                let y = msg.body.get(1).and_then(Value::as_i32).unwrap_or(0);
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("ContextMenu({x},{y})"));
                Ok(Vec::new())
            }
            (Some("org.freedesktop.DBus.Properties"), Some("Get")) => {
                let prop = msg.body.get(1).and_then(Value::as_str).unwrap_or("");
                let val = match prop {
                    "Id" => {
                        std::thread::sleep(Duration::from_millis(750));
                        Value::String("fake-item".into())
                    }
                    "Title" => Value::String(self.title.lock().unwrap().clone()),
                    "Status" => Value::String("Active".into()),
                    "IconName" => Value::String("fake-icon".into()),
                    "IconPixmap" => Value::Array(
                        "(iiay)".into(),
                        vec![Value::Struct(vec![
                            Value::Int32(2),
                            Value::Int32(2),
                            Value::Array(
                                "y".into(),
                                [0xffu8, 0xff, 0x00, 0x00]
                                    .repeat(4)
                                    .into_iter()
                                    .map(Value::Byte)
                                    .collect(),
                            ),
                        ])],
                    ),
                    "Menu" => Value::ObjectPath(OwnedObjectPath::try_from("/").unwrap()),
                    _ => {
                        return Err((
                            "org.freedesktop.DBus.Error.UnknownProperty".into(),
                            format!("unknown property {prop}"),
                        ));
                    }
                };
                Ok(vec![Value::Variant(Box::new(val))])
            }
            _ => Err((
                "org.freedesktop.DBus.Error.UnknownMethod".into(),
                format!("unknown method {:?}.{:?}", msg.interface, msg.member),
            )),
        }
    }
}

fn wait_for(condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
#[ignore = "needs a session bus with a free StatusNotifierWatcher name"]
fn watcher_registers_item_and_delivers_activate() {
    let Some(tray) = tessera_shell::tray::spawn() else {
        eprintln!("no session bus or watcher name taken; skipping");
        return;
    };

    // Client connection serving the fake item at its default path.
    let calls = Arc::new(Mutex::new(Vec::new()));
    let title = Arc::new(Mutex::new("Fake Item".to_string()));
    let client = Connection::session().unwrap();
    client.register_object(
        "/StatusNotifierItem",
        Arc::new(FakeItem {
            calls: Arc::clone(&calls),
            title: Arc::clone(&title),
        }),
    );
    client.request_name("org.example.FakeItem").unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let worker_conn = client.clone();
    let worker_stop = Arc::clone(&stop);
    let _worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Acquire) {
            if worker_conn
                .process_once(Some(Duration::from_millis(20)))
                .is_err()
            {
                break;
            }
        }
    });

    let watcher = Proxy::new(
        &client,
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
    );
    let register_started = Instant::now();
    watcher
        .call::<_, ()>("RegisterStatusNotifierItem", &("org.example.FakeItem",))
        .unwrap();
    assert!(
        register_started.elapsed() < Duration::from_millis(500),
        "registration waited for item properties"
    );

    // The item shows up in the snapshot, mirrored from the fake properties.
    let key = "org.example.FakeItem/StatusNotifierItem";
    assert!(wait_for(|| {
        tray.snapshot()
            .lock()
            .unwrap()
            .items
            .iter()
            .any(|item| item.key == key && item.title == "Fake Item")
    }));

    // A property-change signal must refresh the item on the watcher thread.
    // This also proves the signal loop can make blocking D-Bus reads without
    // nesting `block_on`.
    *title.lock().unwrap() = "Updated Item".to_string();
    client
        .emit_signal(
            None,
            "/StatusNotifierItem",
            "org.kde.StatusNotifierItem",
            "NewTitle",
            &(),
        )
        .expect("emit NewTitle");
    assert!(
        wait_for(|| {
            tray.snapshot()
                .lock()
                .unwrap()
                .items
                .iter()
                .any(|item| item.key == key && item.title == "Updated Item")
        }),
        "the property-change signal did not refresh the item"
    );
    {
        let snapshot = tray.snapshot().lock().unwrap();
        let item = snapshot.items.iter().find(|item| item.key == key).unwrap();
        assert_eq!(item.title, "Updated Item");
        assert!(item.is_visible());
        assert!(!item.has_menu);
        match &item.icon {
            TrayIcon::Pixmap(pixmap) => {
                assert_eq!((pixmap.width, pixmap.height), (2, 2));
                // ARGB red (ff ff 00 00) becomes BGRA (00 00 ff ff).
                assert_eq!(&pixmap.bgra[..4], &[0x00, 0x00, 0xff, 0xff]);
            }
            other => panic!("expected a pixmap icon, got {other:?}"),
        }
    }

    // The watcher's own property lists the registration.
    let registered: Vec<String> = watcher
        .get_property("RegisteredStatusNotifierItems")
        .unwrap();
    assert!(registered.contains(&key.to_string()));
    let host_registered: bool = watcher
        .get_property("IsStatusNotifierHostRegistered")
        .unwrap();
    assert!(host_registered);

    // A left click from the shell reaches the item as Activate(x, y).
    tray.send(TrayCommand::Activate {
        key: key.to_string(),
        x: 10,
        y: 20,
    })
    .unwrap();
    assert!(wait_for(|| calls
        .lock()
        .unwrap()
        .contains(&"Activate(10,20)".to_string())));

    // When the item's connection drops, the watcher forgets it. The proxy
    // holds a connection clone, so it must go first.
    drop(watcher);
    drop(client);
    assert!(wait_for(|| !tray
        .snapshot()
        .lock()
        .unwrap()
        .items
        .iter()
        .any(|item| item.key == key)));
}
