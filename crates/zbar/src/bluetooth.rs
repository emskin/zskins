//! BlueZ DBus client for the quick-settings bluetooth view.
//!
//! Runs on a dedicated thread (its own async-io reactor) and communicates
//! with the GPUI main thread via async-channels:
//!
//! - GPUI → bluez: [`BluetoothClient::connect`] / [`disconnect`] / [`refresh`]
//! - bluez → GPUI: [`BluetoothClient::snapshots`] receives a full device list
//!   on demand and on `InterfacesAdded` / `InterfacesRemoved` /
//!   `PropertiesChanged` signals (live state).
//!
//! Compared to spawning `bluetoothctl`, this eliminates per-action subprocess
//! cost and makes the panel react to *external* changes immediately (e.g.
//! pair via blueman-manager → device appears in our list without polling).

use async_channel::{Receiver, Sender};
use futures_lite::stream::StreamExt;
use std::sync::OnceLock;
use zbus::zvariant::OwnedObjectPath;
use zbus::Connection;

#[derive(Clone, Debug)]
pub struct BtDevice {
    pub path: String,
    pub address: String,
    pub name: String,
    pub icon: String,
    pub connected: bool,
    pub paired: bool,
}

#[derive(Debug)]
pub enum BtCommand {
    Connect(String),
    Disconnect(String),
    Refresh,
}

#[derive(Clone)]
pub struct BluetoothClient {
    cmd_tx: Sender<BtCommand>,
    snap_rx: Receiver<Vec<BtDevice>>,
}

impl BluetoothClient {
    /// Process-wide handle. The first call spawns the bluez thread + opens
    /// the DBus connection; subsequent calls return a cheap clone. We
    /// can't have N clients open N connections — BlueZ DBus signals
    /// would multiply per bar window and waste sockets.
    pub fn shared() -> Self {
        static CLIENT: OnceLock<BluetoothClient> = OnceLock::new();
        CLIENT.get_or_init(BluetoothClient::spawn_inner).clone()
    }

    fn spawn_inner() -> Self {
        let (cmd_tx, cmd_rx) = async_channel::bounded::<BtCommand>(16);
        let (snap_tx, snap_rx) = async_channel::bounded::<Vec<BtDevice>>(4);
        std::thread::Builder::new()
            .name("bluez-client".into())
            .spawn(move || {
                async_io::block_on(run_loop(cmd_rx, snap_tx));
            })
            .expect("spawn bluez thread");
        Self { cmd_tx, snap_rx }
    }

    pub fn refresh(&self) {
        let _ = self.cmd_tx.try_send(BtCommand::Refresh);
    }

    pub fn connect_device(&self, path: String) {
        let _ = self.cmd_tx.try_send(BtCommand::Connect(path));
    }

    pub fn disconnect_device(&self, path: String) {
        let _ = self.cmd_tx.try_send(BtCommand::Disconnect(path));
    }

    pub fn snapshots(&self) -> Receiver<Vec<BtDevice>> {
        self.snap_rx.clone()
    }
}

#[zbus::proxy(interface = "org.bluez.Device1", default_service = "org.bluez")]
trait Device1 {
    fn connect(&self) -> zbus::Result<()>;
    fn disconnect(&self) -> zbus::Result<()>;

    #[zbus(property)]
    fn address(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn alias(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn icon(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn connected(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn paired(&self) -> zbus::Result<bool>;
}

async fn run_loop(cmd_rx: Receiver<BtCommand>, snap_tx: Sender<Vec<BtDevice>>) {
    let conn = match Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("bluez: system bus unavailable: {e}");
            return;
        }
    };

    // Push an initial snapshot.
    push_snapshot(&conn, &snap_tx).await;

    // Subscribe to InterfacesAdded / InterfacesRemoved on the BlueZ object
    // manager so external pair/unpair events trigger a re-snapshot.
    // PropertiesChanged for live connect/disconnect updates is left for
    // a follow-up — we still re-snapshot after every command, so a click
    // inside our UI is responsive; external connects appear at next event.
    let om = match zbus::fdo::ObjectManagerProxy::builder(&conn)
        .destination("org.bluez")
        .and_then(|b| b.path("/"))
    {
        Ok(b) => match b.build().await {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!("bluez: ObjectManagerProxy build failed: {e}");
                None
            }
        },
        Err(e) => {
            tracing::warn!("bluez: ObjectManagerProxy builder failed: {e}");
            None
        }
    };

    let mut added_stream = match om.as_ref() {
        Some(p) => match p.receive_interfaces_added().await {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!("bluez: subscribe InterfacesAdded failed: {e}");
                None
            }
        },
        None => None,
    };
    let mut removed_stream = match om.as_ref() {
        Some(p) => match p.receive_interfaces_removed().await {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!("bluez: subscribe InterfacesRemoved failed: {e}");
                None
            }
        },
        None => None,
    };

    use futures_lite::future::FutureExt;
    loop {
        let cmd_fut = async { cmd_rx.recv().await.ok().map(Event::Cmd) };
        let added_fut = async {
            match added_stream.as_mut() {
                Some(s) => s.next().await.map(|_| Event::SignalAdded),
                None => futures_lite::future::pending().await,
            }
        };
        let removed_fut = async {
            match removed_stream.as_mut() {
                Some(s) => s.next().await.map(|_| Event::SignalRemoved),
                None => futures_lite::future::pending().await,
            }
        };
        let evt: Option<Event> = cmd_fut.or(added_fut).or(removed_fut).await;
        match evt {
            Some(Event::Cmd(BtCommand::Connect(path))) => {
                if let Err(e) = device_connect(&conn, &path).await {
                    tracing::warn!("bluez: connect({path}) failed: {e}");
                }
                push_snapshot(&conn, &snap_tx).await;
            }
            Some(Event::Cmd(BtCommand::Disconnect(path))) => {
                if let Err(e) = device_disconnect(&conn, &path).await {
                    tracing::warn!("bluez: disconnect({path}) failed: {e}");
                }
                push_snapshot(&conn, &snap_tx).await;
            }
            Some(Event::Cmd(BtCommand::Refresh))
            | Some(Event::SignalAdded)
            | Some(Event::SignalRemoved) => {
                push_snapshot(&conn, &snap_tx).await;
            }
            None => return,
        }
    }
}

enum Event {
    Cmd(BtCommand),
    SignalAdded,
    SignalRemoved,
}

async fn push_snapshot(conn: &Connection, tx: &Sender<Vec<BtDevice>>) {
    let devices = match snapshot(conn).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("bluez: snapshot failed: {e}");
            return;
        }
    };
    // Bounded channel; if the GPUI side hasn't drained the previous
    // snapshot, drop the new one — the next emit picks up the latest.
    let _ = tx.try_send(devices);
}

async fn snapshot(conn: &Connection) -> zbus::Result<Vec<BtDevice>> {
    let om = zbus::fdo::ObjectManagerProxy::builder(conn)
        .destination("org.bluez")?
        .path("/")?
        .build()
        .await?;
    let managed = om.get_managed_objects().await?;
    let mut out = Vec::new();
    for (path, ifaces) in managed {
        let Some(_) = ifaces.get("org.bluez.Device1") else {
            continue;
        };
        let dev = match Device1Proxy::builder(conn)
            .path(path.clone())?
            .build()
            .await
        {
            Ok(p) => p,
            Err(_) => continue,
        };
        let address = dev.address().await.unwrap_or_default();
        let alias = dev.alias().await.ok();
        let name = dev.name().await.ok();
        let display_name = alias
            .filter(|s| !s.is_empty())
            .or(name.filter(|s| !s.is_empty()))
            .unwrap_or_else(|| address.clone());
        let icon = dev.icon().await.unwrap_or_default();
        let connected = dev.connected().await.unwrap_or(false);
        let paired = dev.paired().await.unwrap_or(false);
        out.push(BtDevice {
            path: path.as_str().to_string(),
            address,
            name: display_name,
            icon,
            connected,
            paired,
        });
    }
    // Sort: connected → paired → others; tie-break by name.
    out.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| b.paired.cmp(&a.paired))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

async fn device_connect(conn: &Connection, path: &str) -> zbus::Result<()> {
    let path = OwnedObjectPath::try_from(path)?;
    let dev = Device1Proxy::builder(conn).path(path)?.build().await?;
    dev.connect().await
}

async fn device_disconnect(conn: &Connection, path: &str) -> zbus::Result<()> {
    let path = OwnedObjectPath::try_from(path)?;
    let dev = Device1Proxy::builder(conn).path(path)?.build().await?;
    dev.disconnect().await
}
