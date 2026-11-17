//! A room between Pitty terminals. One process hosts; the others join it.
//! Messages and drawings stay in memory on those machines. Nothing is written
//! to a server or a database, and the room ends when the host leaves.

use pitty_ui::{sanitize, width};
use serde_json::{Value, json};
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DISCOVERY_PORT: u16 = 47871;
const LINE_LIMIT: usize = 64 * 1024;
const OUTBOX_LIMIT: usize = 256 * 1024;
const MAX_TEXT: usize = 1000;
const MAX_ARTS: usize = 16;
const MAX_SAYS: usize = 80;
const MAX_ROWS: usize = 12;
pub const MAX_COLS: usize = 48;
pub const MAX_X: u16 = 40;
pub const MAX_Y: u16 = 12;
/// The host plus seven guests.
const MAX_GUESTS: usize = 7;
const STAGE_W: usize = 48;
const STAGE_H: usize = 13;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleKind {
    Discover,
    Host,
    Guest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Art {
    pub id: u64,
    pub name: String,
    pub by: String,
    pub x: u16,
    pub y: u16,
    pub rows: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Said {
    pub by: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nearby {
    pub name: String,
    pub address: String,
    pub number: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Room {
    pub name: String,
    pub role: RoleKind,
    pub share: String,
    pub local: String,
    /// Public room number. Zero means this session is not advertising a room.
    /// The number is what other people see; `share` and `local` stay internal.
    pub number: u16,
    pub peers: Vec<String>,
    pub arts: Vec<Art>,
    pub says: Vec<Said>,
    pub nearby: Vec<Nearby>,
    pub selected: Option<u64>,
    pub closed: bool,
    pub discovery: bool,
}

impl Room {
    fn new(name: String, role: RoleKind, share: String, local: String, number: u16) -> Self {
        let peers = if role == RoleKind::Discover {
            Vec::new()
        } else {
            vec![name.clone()]
        };
        Self {
            name,
            role,
            share,
            local,
            number,
            peers,
            arts: Vec::new(),
            says: Vec::new(),
            nearby: Vec::new(),
            selected: None,
            closed: false,
            discovery: false,
        }
    }

    pub fn connected(&self) -> bool {
        matches!(self.role, RoleKind::Host | RoleKind::Guest) && !self.closed
    }
}

#[derive(Clone, Debug)]
pub struct Update {
    pub room: Option<Room>,
    pub notes: Vec<String>,
}

enum Order {
    Say(String),
    Place { name: String, rows: Vec<String> },
    Nudge { id: u64, dx: i16, dy: i16 },
    Pick(u64),
    Shutdown,
}

enum Event {
    Room(Room),
    Note(String),
}

enum Msg {
    Hello(String),
    Say(String),
    Said {
        name: String,
        text: String,
    },
    Art {
        id: u64,
        name: String,
        by: String,
        x: u16,
        y: u16,
        rows: Vec<String>,
        mark: u64,
    },
    Move {
        id: u64,
        x: u16,
        y: u16,
    },
    Bye {
        name: String,
        host: bool,
        text: String,
    },
    Snapshot {
        peers: Vec<String>,
        arts: Vec<Art>,
        says: Vec<Said>,
    },
}

struct Conn {
    stream: TcpStream,
    inbound: Vec<u8>,
    outbound: Vec<u8>,
    name: String,
    ready: bool,
    born: Instant,
    bad_lines: u8,
    dead: bool,
}

impl Conn {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            inbound: Vec::new(),
            outbound: Vec::new(),
            name: String::new(),
            ready: false,
            born: Instant::now(),
            bad_lines: 0,
            dead: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.outbound.len().saturating_add(bytes.len()) > OUTBOX_LIMIT {
            self.dead = true;
            return;
        }
        self.outbound.extend_from_slice(bytes);
    }

    fn pump(&mut self) -> Result<Vec<Msg>, ()> {
        if !self.outbound.is_empty() {
            match self.stream.write(&self.outbound) {
                Ok(0) => return Err(()),
                Ok(sent) => {
                    self.outbound.drain(..sent);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Err(()),
            }
        }
        let mut buf = [0u8; 4096];
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => return Err(()),
                Ok(read) => {
                    if self.inbound.len().saturating_add(read) > LINE_LIMIT {
                        return Err(());
                    }
                    self.inbound.extend_from_slice(&buf[..read]);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Err(()),
            }
        }
        let mut messages = Vec::new();
        while let Some(pos) = self.inbound.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.inbound.drain(..=pos).collect();
            if line.len() > LINE_LIMIT {
                return Err(());
            }
            let mut line = &line[..line.len() - 1];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<Value>(line)
                .ok()
                .as_ref()
                .and_then(incoming)
            {
                Some(message) => messages.push(message),
                None if serde_json::from_slice::<Value>(line).is_err() => {
                    self.bad_lines = self.bad_lines.saturating_add(1);
                    if self.bad_lines >= 4 {
                        return Err(());
                    }
                }
                None => {}
            }
        }
        if self.inbound.len() > LINE_LIMIT {
            return Err(());
        }
        Ok(messages)
    }
}

struct Seen {
    near: Nearby,
    at: Instant,
}

struct Session {
    room: Room,
    listener: Option<TcpListener>,
    guests: Vec<Conn>,
    upstream: Option<Conn>,
    discover: Option<UdpSocket>,
    beacon: Option<UdpSocket>,
    announce: bool,
    next_beacon: Instant,
    announced: bool,
    told_discovery: bool,
    file_at: Instant,
    seen: Vec<Seen>,
    next_id: u64,
    next_mark: u64,
    waiting: Option<u64>,
    orders: Receiver<Order>,
    events: Sender<Event>,
    stop: Arc<AtomicBool>,
    tcp_port: u16,
}

pub struct Link {
    role: RoleKind,
    // Loopback tests join through this address. The room view never shows it.
    #[cfg(test)]
    share: String,
    number: u16,
    orders: Sender<Order>,
    events: Receiver<Event>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Link {
    pub fn discover(name: &str) -> Result<Self, String> {
        Self::spawn(Start {
            name: display_name(name),
            role: RoleKind::Discover,
            share: String::new(),
            local: String::new(),
            listener: None,
            stream: None,
            announce: false,
            discover: true,
            tcp_port: 0,
            number: 0,
        })
    }

    pub fn host(name: &str, bind: &str) -> Result<Self, String> {
        let bind = bind.trim();
        let (spec, wanted) = if bind.is_empty() {
            ("0.0.0.0:0".to_string(), Some(None))
        } else if !bind.contains(':') && bind.chars().all(|ch| ch.is_ascii_digit()) {
            let number: u16 = bind
                .parse()
                .map_err(|_| "Use a room number from 1 to 999.".to_string())?;
            ("0.0.0.0:0".to_string(), Some(Some(number)))
        } else {
            (parse_bind(bind)?, None)
        };
        Self::host_spec(name, &spec, wanted)
    }

    fn host_spec(name: &str, spec: &str, wanted: Option<Option<u16>>) -> Result<Self, String> {
        let listener =
            TcpListener::bind(spec).map_err(|error| format!("Cannot host a room: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("Cannot host a room: {error}"))?;
        let bound = listener
            .local_addr()
            .map_err(|error| format!("Cannot host a room: {error}"))?;
        let number = match wanted {
            None => 0,
            Some(preferred) => claim_room(preferred, bound.port())?,
        };
        let local = format!("127.0.0.1:{}", bound.port());
        let share = share_address(bound);
        let announce = bound.ip().is_unspecified() && number > 0;
        match Self::spawn(Start {
            name: display_name(name),
            role: RoleKind::Host,
            share,
            local,
            listener: Some(listener),
            stream: None,
            announce,
            discover: announce,
            tcp_port: bound.port(),
            number,
        }) {
            Ok(link) => Ok(link),
            Err(error) => {
                remove_room_file(number, bound.port());
                Err(error)
            }
        }
    }

    pub fn join(name: &str, addr: &str) -> Result<Self, String> {
        Self::join_room(name, addr, 0)
    }

    pub fn join_room(name: &str, addr: &str, number: u16) -> Result<Self, String> {
        let spec = parse_join(addr)?;
        let socket = resolve(&spec)?;
        let stream = TcpStream::connect_timeout(&socket, Duration::from_secs(2))
            .map_err(|_| "Cannot join that room.".to_string())?;
        let _ = stream.set_nodelay(true);
        stream
            .set_nonblocking(true)
            .map_err(|_| "Cannot join that room.".to_string())?;
        Self::spawn(Start {
            name: display_name(name),
            role: RoleKind::Guest,
            share: spec,
            local: String::new(),
            listener: None,
            stream: Some(stream),
            announce: false,
            discover: false,
            tcp_port: 0,
            number,
        })
    }

    pub fn role(&self) -> RoleKind {
        self.role
    }

    #[cfg(test)]
    pub fn share(&self) -> &str {
        &self.share
    }

    pub fn room_number(&self) -> u16 {
        self.number
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.role, RoleKind::Host | RoleKind::Guest) && !self.stop.load(Ordering::SeqCst)
    }

    pub fn is_finished(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    pub fn say(&self, text: &str) -> Result<(), String> {
        if !self.is_connected() {
            return Err("Host or join a room first. Nothing is stored on a server.".into());
        }
        let text = clip_text(text)?;
        self.send(Order::Say(text))
    }

    pub fn place(&self, name: &str, rows: Vec<String>) -> Result<(), String> {
        if !self.is_connected() {
            return Err("Host or join a room first. Nothing is stored on a server.".into());
        }
        let rows = wire_rows(&rows).ok_or_else(|| "That drawing cannot be shared.".to_string())?;
        let name = clean_label(name, 24).unwrap_or_else(|| "art".into());
        self.send(Order::Place { name, rows })
    }

    pub fn nudge(&self, id: u64, dx: i16, dy: i16) -> Result<(), String> {
        if !self.is_connected() {
            return Err("Host or join a room first. Nothing is stored on a server.".into());
        }
        self.send(Order::Nudge { id, dx, dy })
    }

    pub fn pick(&self, id: u64) -> Result<(), String> {
        if !self.is_connected() {
            return Err("Host or join a room first. Nothing is stored on a server.".into());
        }
        self.send(Order::Pick(id))
    }

    pub fn poll(&self) -> Update {
        let mut update = Update {
            room: None,
            notes: Vec::new(),
        };
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Room(room) => update.room = Some(room),
                Event::Note(text) => update.notes.push(text),
            }
        }
        update
    }

    fn send(&self, order: Order) -> Result<(), String> {
        self.orders
            .send(order)
            .map_err(|_| "The room ended.".to_string())
    }

    fn spawn(start: Start) -> Result<Self, String> {
        let (order_tx, order_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let role = start.role;
        #[cfg(test)]
        let share = start.share.clone();
        let number = start.number;
        let thread_stop = Arc::clone(&stop);
        let session_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("pitty-peer".into())
            .spawn(move || {
                let _guard = StopGuard(thread_stop);
                run_session(start, order_rx, event_tx, session_stop);
            })
            .map_err(|error| format!("Cannot start peer chat: {error}"))?;
        Ok(Self {
            role,
            #[cfg(test)]
            share,
            number,
            orders: order_tx,
            events: event_rx,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.orders.send(Order::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct StopGuard(Arc<AtomicBool>);
impl Drop for StopGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Start {
    name: String,
    role: RoleKind,
    share: String,
    local: String,
    listener: Option<TcpListener>,
    stream: Option<TcpStream>,
    announce: bool,
    discover: bool,
    tcp_port: u16,
    number: u16,
}

fn run_session(
    start: Start,
    orders: Receiver<Order>,
    events: Sender<Event>,
    stop: Arc<AtomicBool>,
) {
    let _advert = RoomAdvert {
        number: start.number,
        port: start.tcp_port,
    };
    let mut upstream = start.stream.map(Conn::new);
    if let Some(conn) = &mut upstream {
        conn.ready = true;
        if let Some(bytes) = encode(&json!({"v": 1, "kind": "hello", "name": start.name})) {
            conn.push(&bytes);
        }
    }
    let discover = if start.discover {
        listen_discover()
    } else {
        None
    };
    let beacon = if start.announce {
        beacon_socket()
    } else {
        None
    };
    let mut session = Session {
        room: Room::new(
            start.name,
            start.role,
            start.share,
            start.local,
            start.number,
        ),
        listener: start.listener,
        guests: Vec::new(),
        upstream,
        discover,
        beacon,
        announce: start.announce,
        next_beacon: Instant::now(),
        announced: false,
        told_discovery: false,
        file_at: Instant::now(),
        seen: Vec::new(),
        next_id: 1,
        next_mark: 1,
        waiting: None,
        orders,
        events,
        stop,
        tcp_port: start.tcp_port,
    };
    session.room.discovery = session.discover.is_some();
    if start.discover && session.discover.is_none() {
        session.note(
            "Nearby rooms are not visible on this computer. /peer join ROOM still reaches a room here.".into(),
        );
        session.told_discovery = true;
    }
    if !session.publish() {
        return;
    }
    while !session.stop.load(Ordering::SeqCst) {
        if !session.orders_once() {
            break;
        }
        session.accept_guests();
        if !session.pump_all() {
            break;
        }
        session.drain_beacons();
        session.announce_once();
        session.refresh_room_file();
        if session.stop.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    session.goodbye();
}

impl Session {
    fn orders_once(&mut self) -> bool {
        loop {
            match self.orders.try_recv() {
                Ok(Order::Shutdown) => {
                    self.stop.store(true, Ordering::SeqCst);
                    return false;
                }
                Ok(order) => self.handle_order(order),
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => return false,
            }
        }
    }

    fn handle_order(&mut self, order: Order) {
        match order {
            Order::Shutdown => self.stop.store(true, Ordering::SeqCst),
            Order::Say(text) => self.local_say(text),
            Order::Place { name, rows } => self.local_place(name, rows),
            Order::Nudge { id, dx, dy } => self.local_nudge(id, dx, dy),
            Order::Pick(id) => {
                if self.room.arts.iter().any(|art| art.id == id) {
                    self.room.selected = Some(id);
                    self.publish();
                }
            }
        }
    }

    fn local_say(&mut self, text: String) {
        if self.room.role != RoleKind::Host {
            self.send_upstream(&json!({"v": 1, "kind": "say", "text": text}));
            return;
        }
        let name = self.room.name.clone();
        self.accept_say(name, text);
    }

    fn local_place(&mut self, name: String, rows: Vec<String>) {
        let mark = self.next_mark;
        self.next_mark = self.next_mark.wrapping_add(1);
        if self.room.role == RoleKind::Host {
            let by = self.room.name.clone();
            self.add_art(&by, &name, rows, mark, true);
            return;
        }
        self.waiting = Some(mark);
        self.send_upstream(&json!({
            "v": 1,
            "kind": "art",
            "name": name,
            "rows": rows,
            "mark": mark
        }));
    }

    fn local_nudge(&mut self, id: u64, dx: i16, dy: i16) {
        let Some(art) = self.room.arts.iter().find(|art| art.id == id) else {
            return;
        };
        let (x, y) = nudge(art.x, art.y, dx, dy);
        if !self.apply_move(id, x, y) {
            return;
        }
        if self.room.role == RoleKind::Host {
            self.broadcast(&json!({"v": 1, "kind": "move", "id": id, "x": x, "y": y}));
        } else {
            self.send_upstream(&json!({"v": 1, "kind": "move", "id": id, "x": x, "y": y}));
        }
        self.publish();
    }

    fn send_upstream(&mut self, value: &Value) {
        let Some(bytes) = encode(value) else {
            return;
        };
        if let Some(conn) = &mut self.upstream {
            conn.push(&bytes);
        }
    }

    fn accept_say(&mut self, name: String, text: String) {
        let Ok(text) = clip_text(&text) else {
            return;
        };
        self.room.says.push(Said {
            by: name.clone(),
            text: text.clone(),
        });
        self.trim_says();
        if self.room.role == RoleKind::Host {
            self.broadcast(&json!({"v": 1, "kind": "say", "name": name, "text": text}));
        }
        self.publish();
    }

    fn add_art(&mut self, by: &str, name: &str, rows: Vec<String>, mark: u64, select: bool) {
        let Some(rows) = wire_rows(&rows) else {
            return;
        };
        if self.room.arts.len() >= MAX_ARTS {
            self.refuse_art(by);
            return;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let index = self.room.arts.len() as u16;
        let art = Art {
            id,
            name: clean_label(name, 24).unwrap_or_else(|| "art".into()),
            by: clean_label(by, 40).unwrap_or_else(|| "pitty".into()),
            x: (2 + index.saturating_mul(3)).min(MAX_X),
            y: (1 + index).min(MAX_Y),
            rows,
        };
        self.broadcast(&art_value(&art, mark));
        if select || self.room.selected.is_none() {
            self.room.selected = Some(id);
        }
        self.room.arts.push(art);
        self.publish();
    }

    fn refuse_art(&mut self, who: &str) {
        let text = "The room has 16 drawings.".to_string();
        if who == self.room.name {
            self.note(text);
            return;
        }
        let Some(bytes) = encode(&json!({"v": 1, "kind": "say", "name": "room", "text": text}))
        else {
            return;
        };
        if let Some(conn) = self
            .guests
            .iter_mut()
            .find(|conn| conn.name == who && conn.ready)
        {
            conn.push(&bytes);
        }
    }

    fn accept_guests(&mut self) {
        loop {
            let accepted = self.listener.as_ref().map(TcpListener::accept);
            match accepted {
                Some(Ok((stream, _))) => self.take_guest(stream),
                Some(Err(error)) if error.kind() == ErrorKind::WouldBlock => break,
                Some(Err(error)) if error.kind() == ErrorKind::Interrupted => continue,
                _ => break,
            }
        }
    }

    fn take_guest(&mut self, stream: TcpStream) {
        let _ = stream.set_nodelay(true);
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        if self.guests.len() >= MAX_GUESTS {
            let mut conn = Conn::new(stream);
            if let Some(bytes) = encode(&json!({
                "v": 1,
                "kind": "bye",
                "host": true,
                "name": "room",
                "text": "This room has 8 people."
            })) {
                conn.push(&bytes);
                let _ = conn.pump();
            }
            return;
        }
        self.guests.push(Conn::new(stream));
    }

    fn pump_all(&mut self) -> bool {
        let upstream = self.upstream.as_mut().map(|conn| conn.pump());
        match upstream {
            Some(Err(())) => {
                self.finish("The host left. The room ended.".into());
                return false;
            }
            Some(Ok(messages)) => {
                for message in messages {
                    self.on_upstream(message);
                    if self.stop.load(Ordering::SeqCst) {
                        return false;
                    }
                }
            }
            None => {}
        }
        let mut incoming = Vec::new();
        let mut dead = Vec::new();
        for (index, conn) in self.guests.iter_mut().enumerate() {
            if !conn.ready && conn.born.elapsed() > Duration::from_secs(2) {
                dead.push(index);
                continue;
            }
            match conn.pump() {
                Ok(messages) => incoming.push((index, messages)),
                Err(()) => dead.push(index),
            }
        }
        for (index, messages) in incoming {
            if dead.contains(&index) {
                continue;
            }
            for message in messages {
                self.on_guest(index, message);
            }
            if self.guests.get(index).is_some_and(|conn| conn.dead) && !dead.contains(&index) {
                dead.push(index);
            }
        }
        dead.sort_unstable();
        dead.dedup();
        for index in dead.into_iter().rev() {
            self.drop_guest(index);
        }
        true
    }

    fn on_guest(&mut self, index: usize, message: Msg) {
        if index >= self.guests.len() {
            return;
        }
        match message {
            Msg::Hello(name) if !self.guests[index].ready => self.welcome(index, name),
            Msg::Say(text) if self.guests[index].ready => {
                let name = self.guests[index].name.clone();
                self.accept_say(name, text);
            }
            Msg::Art {
                id,
                name,
                rows,
                mark,
                ..
            } if id == 0 && self.guests[index].ready => {
                let by = self.guests[index].name.clone();
                self.add_art(&by, &name, rows, mark, false);
            }
            Msg::Move { id, x, y } if self.guests[index].ready => self.on_move(id, x, y),
            Msg::Bye { name, .. } if self.guests[index].ready => {
                self.guests[index].dead = true;
                let _ = name;
            }
            _ => {}
        }
    }

    fn on_upstream(&mut self, message: Msg) {
        match message {
            Msg::Snapshot { peers, arts, says } => self.apply_snapshot(peers, arts, says),
            Msg::Said { name, text } => self.accept_say(name, text),
            Msg::Hello(name) => {
                if self.room.peers.len() < MAX_GUESTS + 1
                    && !self.room.peers.iter().any(|peer| peer == &name)
                {
                    self.room.peers.push(name);
                    self.publish();
                }
            }
            Msg::Art {
                id,
                name,
                by,
                x,
                y,
                rows,
                mark,
            } if id > 0 => {
                self.upsert_art(Art {
                    id,
                    name,
                    by,
                    x,
                    y,
                    rows,
                });
                if self.waiting == Some(mark) && mark != 0 {
                    self.room.selected = Some(id);
                    self.waiting = None;
                } else if self.room.selected.is_none() {
                    self.room.selected = Some(id);
                }
                self.publish();
            }
            Msg::Move { id, x, y } => self.on_move(id, x, y),
            Msg::Bye { host, text, name } => {
                if host {
                    let reason = if text.is_empty() {
                        "The host left. The room ended.".into()
                    } else {
                        text
                    };
                    self.finish(reason);
                } else {
                    self.remove_guest_name(&name);
                    self.publish();
                }
            }
            Msg::Say(_) | Msg::Art { .. } => {}
        }
    }

    fn welcome(&mut self, index: usize, name: String) {
        if index >= self.guests.len() {
            return;
        }
        let name = distinct_name(&self.room.peers, &name);
        self.guests[index].name = name.clone();
        self.guests[index].ready = true;
        self.room.peers.push(name.clone());
        let joined = format!("{name} joined");
        self.room.says.push(Said {
            by: "room".into(),
            text: joined.clone(),
        });
        self.trim_says();
        self.send_snapshot(index);
        self.broadcast_except(Some(index), &json!({"v": 1, "kind": "hello", "name": name}));
        self.broadcast_except(
            Some(index),
            &json!({"v": 1, "kind": "say", "name": "room", "text": joined}),
        );
        self.publish();
    }

    fn on_move(&mut self, id: u64, x: u16, y: u16) {
        let x = x.min(MAX_X);
        let y = y.min(MAX_Y);
        if !self.apply_move(id, x, y) {
            return;
        }
        if self.room.role == RoleKind::Host {
            self.broadcast(&json!({"v": 1, "kind": "move", "id": id, "x": x, "y": y}));
        }
        self.publish();
    }

    fn apply_move(&mut self, id: u64, x: u16, y: u16) -> bool {
        let Some(index) = self.room.arts.iter().position(|art| art.id == id) else {
            return false;
        };
        let mut art = self.room.arts.remove(index);
        art.x = x.min(MAX_X);
        art.y = y.min(MAX_Y);
        self.room.arts.push(art);
        true
    }

    fn upsert_art(&mut self, art: Art) {
        if let Some(index) = self
            .room
            .arts
            .iter()
            .position(|existing| existing.id == art.id)
        {
            self.room.arts.remove(index);
        } else if self.room.arts.len() >= MAX_ARTS {
            return;
        }
        self.room.arts.push(art);
    }

    fn apply_snapshot(&mut self, peers: Vec<String>, arts: Vec<Art>, says: Vec<Said>) {
        let selected = self
            .room
            .selected
            .filter(|id| arts.iter().any(|art| art.id == *id));
        self.room.peers = peers.into_iter().take(MAX_GUESTS + 1).collect();
        self.room.arts = arts.into_iter().take(MAX_ARTS).collect();
        self.room.says = says.into_iter().take(MAX_SAYS).collect();
        self.room.selected = selected.or_else(|| self.room.arts.first().map(|art| art.id));
        self.publish();
    }

    fn drop_guest(&mut self, index: usize) {
        if index >= self.guests.len() {
            return;
        }
        let conn = self.guests.remove(index);
        if !conn.ready || conn.name.is_empty() {
            return;
        }
        self.remove_guest_name(&conn.name);
        let text = format!("{} left", conn.name);
        self.room.says.push(Said {
            by: "room".into(),
            text: text.clone(),
        });
        self.trim_says();
        self.broadcast(&json!({"v": 1, "kind": "bye", "name": conn.name}));
        self.broadcast(&json!({"v": 1, "kind": "say", "name": "room", "text": text}));
        self.publish();
    }

    fn remove_guest_name(&mut self, name: &str) {
        if let Some(offset) = self.room.peers.iter().skip(1).position(|peer| peer == name) {
            self.room.peers.remove(offset + 1);
        }
    }

    fn send_snapshot(&mut self, index: usize) {
        while encode(&self.snapshot_value()).is_none() && !self.room.says.is_empty() {
            self.room.says.remove(0);
        }
        let Some(bytes) = encode(&self.snapshot_value()) else {
            return;
        };
        if let Some(conn) = self.guests.get_mut(index) {
            conn.push(&bytes);
        }
    }

    fn snapshot_value(&self) -> Value {
        json!({
            "v": 1,
            "kind": "snapshot",
            "peers": self.room.peers,
            "arts": self.room.arts.iter().map(|art| art_value(art, 0)).collect::<Vec<_>>(),
            "says": self.room.says.iter().map(|say| json!({"by": say.by, "text": say.text})).collect::<Vec<_>>(),
        })
    }

    fn broadcast(&mut self, value: &Value) {
        self.broadcast_except(None, value);
    }

    fn broadcast_except(&mut self, skip: Option<usize>, value: &Value) {
        let Some(bytes) = encode(value) else {
            return;
        };
        for (index, conn) in self.guests.iter_mut().enumerate() {
            if Some(index) == skip || !conn.ready {
                continue;
            }
            conn.push(&bytes);
        }
    }

    fn trim_says(&mut self) {
        if self.room.says.len() > MAX_SAYS {
            let extra = self.room.says.len() - MAX_SAYS;
            self.room.says.drain(0..extra);
        }
    }

    fn drain_beacons(&mut self) {
        let Some(sock) = &self.discover else {
            return;
        };
        let mut buf = [0u8; 512];
        let mut changed = false;
        loop {
            match sock.recv_from(&mut buf) {
                Ok((read, source)) => {
                    if let Some(near) = parse_beacon(&buf[..read], source) {
                        if near.address == self.room.share || near.address == self.room.local {
                            continue;
                        }
                        self.seen.retain(|seen| seen.near.address != near.address);
                        self.seen.push(Seen {
                            near,
                            at: Instant::now(),
                        });
                        changed = true;
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let fresh = Instant::now();
        let before = self.seen.len();
        self.seen
            .retain(|seen| fresh.duration_since(seen.at) < Duration::from_secs(4));
        if self.seen.len() != before {
            changed = true;
        }
        if changed {
            self.room.nearby = self.seen.iter().map(|seen| seen.near.clone()).collect();
            self.publish();
        }
    }

    fn announce_once(&mut self) {
        if !self.announce || self.tcp_port == 0 {
            return;
        }
        if self.announced && self.next_beacon.elapsed() < Duration::from_secs(1) {
            return;
        }
        let Some(sock) = &self.beacon else {
            return;
        };
        let name = beacon_name(&self.room.name);
        let payload = format!("PITTY1\t{}\t{name}\t{}", self.tcp_port, self.room.number);
        match sock.send_to(payload.as_bytes(), (Ipv4Addr::BROADCAST, DISCOVERY_PORT)) {
            Ok(_) => {
                self.announced = true;
                self.next_beacon = Instant::now();
            }
            Err(error)
                if error.kind() == ErrorKind::WouldBlock
                    || error.kind() == ErrorKind::Interrupted => {}
            Err(_) => {
                self.announce = false;
                if !self.told_discovery {
                    let number = self.room.number;
                    self.note(format!(
                        "Other computers cannot see room {number} on this network. A Pitty on this computer joins with /peer join {number}."
                    ));
                    self.told_discovery = true;
                }
            }
        }
    }

    fn refresh_room_file(&mut self) {
        if self.room.number == 0 || self.tcp_port == 0 {
            return;
        }
        if self.file_at.elapsed() < Duration::from_secs(1) {
            return;
        }
        rewrite_room_file(self.room.number, self.tcp_port);
        self.file_at = Instant::now();
    }

    fn finish(&mut self, reason: String) {
        self.room.closed = true;
        self.note(reason);
        self.publish();
        self.stop.store(true, Ordering::SeqCst);
    }

    fn goodbye(&mut self) {
        if self.room.role != RoleKind::Host {
            return;
        }
        let name = self.room.name.clone();
        self.broadcast(&json!({
            "v": 1,
            "kind": "bye",
            "host": true,
            "name": name,
            "text": "The host left. The room ended."
        }));
        for conn in &mut self.guests {
            let _ = conn.pump();
        }
    }

    fn publish(&mut self) -> bool {
        self.events.send(Event::Room(self.room.clone())).is_ok()
    }

    fn note(&mut self, text: String) {
        let _ = self.events.send(Event::Note(text));
    }
}

pub fn nudge(x: u16, y: u16, dx: i16, dy: i16) -> (u16, u16) {
    let x = (i32::from(x) + i32::from(dx)).clamp(0, i32::from(MAX_X)) as u16;
    let y = (i32::from(y) + i32::from(dy)).clamp(0, i32::from(MAX_Y)) as u16;
    (x, y)
}

pub fn direction(name: &str) -> Option<(i16, i16)> {
    match name.trim().to_ascii_lowercase().as_str() {
        "left" | "h" => Some((-1, 0)),
        "right" | "l" => Some((1, 0)),
        "up" | "k" => Some((0, -1)),
        "down" | "j" => Some((0, 1)),
        _ => None,
    }
}

pub fn builtin(name: &str) -> Option<Vec<String>> {
    let rows: &[&str] = match name.trim().to_ascii_lowercase().as_str() {
        "cat" => &[r" /\_/\", "( o.o )", " > ^ <"],
        "ship" => &["  |~", r" /|\", "/_|_\\"],
        "crab" => &[" m  m", r"(o\/o)", " (  )", "/|  |\\"],
        "spark" => &["  *", " * *", "*****", " * *", "  *"],
        _ => return None,
    };
    Some(rows.iter().map(|row| (*row).to_string()).collect())
}

pub fn prepare_rows(rows: &[String]) -> Result<Vec<String>, String> {
    let rows: Vec<String> = rows
        .iter()
        .map(|row| sanitize(row).replace('\n', ""))
        .collect();
    let mut end = rows.len();
    while end > 0 && rows[end - 1].trim().is_empty() {
        end -= 1;
    }
    let rows = rows[..end].to_vec();
    if rows.is_empty() || rows.iter().all(|row| row.trim().is_empty()) {
        return Err("Draw at least one line. Alt+Enter starts a new line.".into());
    }
    if rows.len() > MAX_ROWS {
        return Err("ASCII art can be at most 12 lines.".into());
    }
    if rows.iter().any(|row| width(row) > MAX_COLS) {
        return Err("Each line can be at most 48 columns.".into());
    }
    if rows.iter().any(|row| row.chars().any(|c| c == '\u{1b}')) {
        return Err("Art cannot contain terminal control sequences.".into());
    }
    Ok(rows)
}

pub fn canvas(arts: &[Art]) -> Vec<String> {
    let mut grid = vec![vec![' '; STAGE_W]; STAGE_H];
    for art in arts {
        for (dy, row) in art.rows.iter().enumerate() {
            let Some(line) = grid.get_mut(art.y as usize + dy) else {
                break;
            };
            let mut x = art.x as usize;
            for ch in row.chars() {
                let cell = width(&ch.to_string()).max(1);
                if x >= STAGE_W {
                    break;
                }
                if ch != ' ' {
                    line[x] = ch;
                }
                x += cell;
            }
        }
    }
    grid.into_iter()
        .map(|line| line.into_iter().collect::<String>().trim_end().to_string())
        .collect()
}

fn clip_text(text: &str) -> Result<String, String> {
    let text: String = sanitize(text)
        .chars()
        .filter(|ch| *ch != '\n')
        .take(MAX_TEXT)
        .collect();
    let text = text.trim();
    if text.is_empty() {
        Err("Say something visible.".into())
    } else {
        Ok(text.to_string())
    }
}

fn wire_rows(rows: &[String]) -> Option<Vec<String>> {
    if rows.is_empty() || rows.len() > MAX_ROWS {
        return None;
    }
    let rows: Vec<String> = rows
        .iter()
        .map(|row| clip_cols(&sanitize(row).replace('\n', ""), MAX_COLS))
        .collect();
    if rows.iter().all(|row| row.trim().is_empty()) {
        None
    } else {
        Some(rows)
    }
}

fn clip_cols(text: &str, columns: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for ch in text.chars() {
        let cell = width(&ch.to_string());
        if count + cell > columns {
            break;
        }
        out.push(ch);
        count += cell;
    }
    out
}

fn clean_label(text: &str, max: usize) -> Option<String> {
    let text: String = sanitize(text)
        .chars()
        .filter(|ch| *ch != '\n' && *ch != '\t')
        .take(max)
        .collect();
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn display_name(name: &str) -> String {
    clean_label(name, 40).unwrap_or_else(|| "pitty".into())
}

/// Two Pittys on one computer share a profile name. The host keeps the first
/// spelling and labels each later copy "Name (2)", "Name (3)", and so on.
fn distinct_name(taken: &[String], name: &str) -> String {
    if !taken.iter().any(|peer| peer == name) {
        return name.to_string();
    }
    for number in 2..100 {
        let suffix = format!(" ({number})");
        let keep = 40usize.saturating_sub(suffix.chars().count());
        let stem: String = name.chars().take(keep).collect();
        let candidate = format!("{}{suffix}", stem.trim_end());
        if candidate.chars().count() <= 40 && !taken.iter().any(|peer| peer == &candidate) {
            return candidate;
        }
    }
    name.to_string()
}

fn beacon_name(name: &str) -> String {
    display_name(name)
        .chars()
        .filter(|ch| *ch != '\t')
        .collect()
}

fn art_value(art: &Art, mark: u64) -> Value {
    json!({
        "v": 1,
        "kind": "art",
        "id": art.id,
        "name": art.name,
        "by": art.by,
        "x": art.x,
        "y": art.y,
        "rows": art.rows,
        "mark": mark,
    })
}

fn encode(value: &Value) -> Option<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).ok()?;
    if bytes.len() + 1 > LINE_LIMIT {
        return None;
    }
    bytes.push(b'\n');
    Some(bytes)
}

fn incoming(value: &Value) -> Option<Msg> {
    if value.get("v")?.as_u64()? != 1 {
        return None;
    }
    match value.get("kind")?.as_str()? {
        "hello" => Some(Msg::Hello(clean_label(value.get("name")?.as_str()?, 40)?)),
        "say" => {
            let text = clip_text(value.get("text")?.as_str()?).ok()?;
            match value.get("name").and_then(Value::as_str) {
                Some(name) => Some(Msg::Said {
                    name: clean_label(name, 40)?,
                    text,
                }),
                None => Some(Msg::Say(text)),
            }
        }
        "art" => Some(Msg::Art {
            id: value.get("id").and_then(Value::as_u64).unwrap_or(0),
            name: clean_label(
                value.get("name").and_then(Value::as_str).unwrap_or("art"),
                24,
            )
            .unwrap_or_else(|| "art".into()),
            by: clean_label(
                value.get("by").and_then(Value::as_str).unwrap_or("pitty"),
                40,
            )
            .unwrap_or_else(|| "pitty".into()),
            x: value
                .get("x")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(u64::from(MAX_X)) as u16,
            y: value
                .get("y")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(u64::from(MAX_Y)) as u16,
            rows: raw_rows(value.get("rows")?)?,
            mark: value.get("mark").and_then(Value::as_u64).unwrap_or(0),
        }),
        "move" => Some(Msg::Move {
            id: value.get("id")?.as_u64()?,
            x: value.get("x")?.as_u64()?.min(u64::from(MAX_X)) as u16,
            y: value.get("y")?.as_u64()?.min(u64::from(MAX_Y)) as u16,
        }),
        "bye" => Some(Msg::Bye {
            name: clean_label(
                value.get("name").and_then(Value::as_str).unwrap_or("room"),
                40,
            )
            .unwrap_or_else(|| "room".into()),
            host: value.get("host").and_then(Value::as_bool).unwrap_or(false),
            text: value
                .get("text")
                .and_then(Value::as_str)
                .and_then(|text| clip_text(text).ok())
                .unwrap_or_default(),
        }),
        "snapshot" => Some(Msg::Snapshot {
            peers: string_list(value.get("peers")?, MAX_GUESTS + 1)?,
            arts: value
                .get("arts")?
                .as_array()?
                .iter()
                .filter_map(|art| match incoming(art) {
                    Some(Msg::Art {
                        id,
                        name,
                        by,
                        x,
                        y,
                        rows,
                        ..
                    }) if id > 0 => Some(Art {
                        id,
                        name,
                        by,
                        x,
                        y,
                        rows,
                    }),
                    _ => None,
                })
                .take(MAX_ARTS)
                .collect(),
            says: value
                .get("says")?
                .as_array()?
                .iter()
                .filter_map(|say| {
                    Some(Said {
                        by: clean_label(say.get("by")?.as_str()?, 40)?,
                        text: clip_text(say.get("text")?.as_str()?).ok()?,
                    })
                })
                .take(MAX_SAYS)
                .collect(),
        }),
        _ => None,
    }
}

fn raw_rows(value: &Value) -> Option<Vec<String>> {
    let rows = value
        .as_array()?
        .iter()
        .filter_map(|item| Some(sanitize(item.as_str()?).replace('\n', "")))
        .take(MAX_ROWS + 1)
        .collect::<Vec<_>>();
    wire_rows(&rows)
}

fn string_list(value: &Value, max: usize) -> Option<Vec<String>> {
    Some(
        value
            .as_array()?
            .iter()
            .filter_map(|item| clean_label(item.as_str()?, 1000))
            .take(max)
            .collect(),
    )
}

fn parse_bind(spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok("0.0.0.0:0".into());
    }
    if spec.chars().all(|ch| ch.is_ascii_digit()) {
        let port: u16 = spec
            .parse()
            .map_err(|_| "Use a port from 0 to 65535.".to_string())?;
        return Ok(format!("0.0.0.0:{port}"));
    }
    check_addr(spec)?;
    Ok(spec.to_string())
}

fn parse_join(spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    check_addr(spec)?;
    let (_, port) = split_host_port(spec)?;
    if port == 0 {
        return Err("Use host:port with a real port.".into());
    }
    Ok(spec.to_string())
}

fn check_addr(spec: &str) -> Result<(), String> {
    if spec.is_empty()
        || spec.len() > 200
        || spec.chars().any(|ch| {
            ch.is_whitespace() || ch.is_control() || matches!(ch, '@' | '/' | '?' | '#' | '\\')
        })
    {
        return Err("Use host:port.".into());
    }
    split_host_port(spec).map(|_| ())
}

fn split_host_port(spec: &str) -> Result<(String, u16), String> {
    if let Some(rest) = spec.strip_prefix('[') {
        let (host, port) = rest
            .split_once("]:")
            .ok_or_else(|| "Use [ipv6]:port.".to_string())?;
        if host.is_empty() {
            return Err("Use [ipv6]:port.".into());
        }
        return Ok((host.to_string(), parse_port(port)?));
    }
    if spec.matches(':').count() != 1 {
        return Err("Use host:port, or [ipv6]:port.".into());
    }
    let (host, port) = spec
        .rsplit_once(':')
        .ok_or_else(|| "Use host:port.".to_string())?;
    if host.is_empty() {
        return Err("Use host:port.".into());
    }
    Ok((host.to_string(), parse_port(port)?))
}

fn parse_port(text: &str) -> Result<u16, String> {
    text.parse::<u16>()
        .map_err(|_| "Use a port from 0 to 65535.".to_string())
}

fn resolve(spec: &str) -> Result<SocketAddr, String> {
    spec.to_socket_addrs()
        .map_err(|error| format!("Cannot find {spec}: {error}"))?
        .next()
        .ok_or_else(|| format!("Cannot find {spec}."))
}

fn share_address(bound: SocketAddr) -> String {
    let port = bound.port();
    if bound.ip().is_unspecified() {
        format!("{}:{port}", lan_ip().unwrap_or_else(|| "127.0.0.1".into()))
    } else if bound.ip().is_ipv4() {
        format!("{}:{port}", bound.ip())
    } else {
        format!("[{}]:{port}", bound.ip())
    }
}

fn lan_ip() -> Option<String> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    sock.connect("192.0.2.1:9").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    if ip.is_unspecified() {
        None
    } else {
        Some(ip.to_string())
    }
}

fn listen_discover() -> Option<UdpSocket> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).ok()?;
    sock.set_nonblocking(true).ok()?;
    Some(sock)
}

fn beacon_socket() -> Option<UdpSocket> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    sock.set_broadcast(true).ok()?;
    sock.set_nonblocking(true).ok()?;
    Some(sock)
}

fn parse_beacon(packet: &[u8], source: SocketAddr) -> Option<Nearby> {
    if packet.len() > 200 {
        return None;
    }
    let text = std::str::from_utf8(packet)
        .ok()?
        .trim_end_matches(['\n', '\r']);
    let mut parts = text.split('\t');
    if parts.next()? != "PITTY1" {
        return None;
    }
    let port: u16 = parts.next()?.parse().ok()?;
    if port == 0 {
        return None;
    }
    let name = clean_label(parts.next()?, 40)?;
    let number: u16 = parts.next()?.parse().ok()?;
    if !(1..=999).contains(&number) || parts.next().is_some() {
        return None;
    }
    let address = match source.ip() {
        std::net::IpAddr::V4(ip) => format!("{ip}:{port}"),
        std::net::IpAddr::V6(ip) => format!("[{ip}]:{port}"),
    };
    Some(Nearby {
        name,
        address,
        number,
    })
}

pub fn locate_room(number: u16) -> Result<String, String> {
    if !(1..=999).contains(&number) {
        return Err("Use a room number from 1 to 999.".into());
    }
    if let Some(port) = read_local_room(number) {
        return Ok(format!("127.0.0.1:{port}"));
    }
    listen_for_room(number).ok_or_else(|| format!("No room {number} nearby."))
}

fn listen_for_room(number: u16) -> Option<String> {
    let sock = listen_discover()?;
    let deadline = Instant::now() + Duration::from_millis(1200);
    let mut buf = [0u8; 512];
    while Instant::now() < deadline {
        match sock.recv_from(&mut buf) {
            Ok((read, source)) => {
                if let Some(near) = parse_beacon(&buf[..read], source)
                    && near.number == number
                {
                    return Some(near.address);
                }
            }
            Err(error)
                if error.kind() == ErrorKind::WouldBlock
                    || error.kind() == ErrorKind::Interrupted =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
    None
}

struct RoomAdvert {
    number: u16,
    port: u16,
}

impl Drop for RoomAdvert {
    fn drop(&mut self) {
        remove_room_file(self.number, self.port);
    }
}

fn rooms_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("pitty-rooms")
}

fn room_path(number: u16) -> std::path::PathBuf {
    rooms_dir().join(number.to_string())
}

fn ensure_rooms_dir() -> std::io::Result<()> {
    let dir = rooms_dir();
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn read_local_room(number: u16) -> Option<u16> {
    let path = room_path(number);
    let meta = fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > 16 {
        return None;
    }
    let age = meta.modified().ok()?.elapsed().ok()?;
    if age > Duration::from_secs(4) {
        return None;
    }
    read_room_port(number)
}

fn read_room_port(number: u16) -> Option<u16> {
    let text = fs::read_to_string(room_path(number)).ok()?;
    let port = text.trim().parse::<u16>().ok()?;
    (port > 0).then_some(port)
}

fn remove_room_file(number: u16, port: u16) {
    if number > 0 && read_room_port(number) == Some(port) {
        let _ = fs::remove_file(room_path(number));
    }
}

fn claim_room(preferred: Option<u16>, port: u16) -> Result<u16, String> {
    let numbers = match preferred {
        Some(number) => {
            if !(1..=999).contains(&number) {
                return Err("Use a room number from 1 to 999.".into());
            }
            if read_local_room(number).is_some() {
                return Err(format!("Room {number} is already open on this computer."));
            }
            vec![number]
        }
        None => room_candidates(),
    };
    ensure_rooms_dir().map_err(|error| format!("Cannot open a room on this computer: {error}"))?;
    for number in numbers {
        if read_local_room(number).is_some() {
            continue;
        }
        if room_path(number).exists() {
            let _ = fs::remove_file(room_path(number));
        }
        if create_room_file(number, port).is_ok() {
            return Ok(number);
        }
        if preferred.is_some() {
            return Err(format!("Room {number} is already open on this computer."));
        }
    }
    Err("Every room number from 1 to 99 is already open on this computer.".into())
}

fn room_candidates() -> Vec<u16> {
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
        ^ std::process::id();
    let mut numbers = Vec::with_capacity(99);
    for _ in 0..99 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let number = ((seed % 99) + 1) as u16;
        if !numbers.contains(&number) {
            numbers.push(number);
        }
    }
    for number in 1..=99u16 {
        if !numbers.contains(&number) {
            numbers.push(number);
        }
    }
    numbers
}

fn create_room_file(number: u16, port: u16) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(room_path(number))?;
    writeln!(file, "{port}")?;
    file.sync_all()?;
    Ok(())
}

fn rewrite_room_file(number: u16, port: u16) {
    if number == 0 || port == 0 {
        return;
    }
    if fs::write(room_path(number), format!("{port}\n")).is_err() {
        let _ = ensure_rooms_dir();
        let _ = create_room_file(number, port);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_clips_text_and_strips_terminal_controls() {
        let noisy = format!("hi\u{1b}[31m{}", "z".repeat(2000));
        let text = clip_text(&noisy).unwrap();
        assert_eq!(text.chars().count(), MAX_TEXT);
        assert!(text.starts_with("hiz"));
        assert!(!text.contains('\u{1b}'));
        assert_eq!(clip_text("\u{1b}[31mred").unwrap(), "red");
        assert!(clip_text("\u{1b}[31m").is_err());
        assert!(incoming(&json!({"v": 1, "kind": "run", "cmd": "id"})).is_none());
        assert!(incoming(&json!({"v": 1, "kind": "shell", "text": "rm -rf"})).is_none());
        let art = incoming(&json!({
            "v": 1,
            "kind": "art",
            "name": "wide",
            "rows": [format!("\u{1b}[32m{}", "w".repeat(80))]
        }));
        let Msg::Art { rows, .. } = art.unwrap() else {
            panic!("art");
        };
        assert_eq!(rows[0].chars().count(), MAX_COLS);
        assert!(!rows[0].contains('\u{1b}'));
        assert!(prepare_rows(&["x".repeat(49)]).is_err());
        let too_tall = vec!["line".to_string(); 13];
        assert!(prepare_rows(&too_tall).is_err());
        let drawn = prepare_rows(&["  \u{1b}[31mcat".into()]).unwrap();
        assert_eq!(drawn, vec!["  cat".to_string()]);
    }

    #[test]
    fn nudge_clamps_to_the_stage() {
        assert_eq!(nudge(0, 0, -5, -3), (0, 0));
        assert_eq!(nudge(MAX_X, MAX_Y, 4, 2), (MAX_X, MAX_Y));
        assert_eq!(nudge(1, 1, 1, -1), (2, 0));
        assert_eq!(direction("k"), Some((0, -1)));
        assert_eq!(direction("h"), Some((-1, 0)));
        assert!(direction("nope").is_none());
        let cat = builtin("CAT").unwrap();
        let stage = canvas(&[Art {
            id: 1,
            name: "cat".into(),
            by: "ada".into(),
            x: 0,
            y: 0,
            rows: cat,
        }]);
        assert!(stage.iter().any(|line| line.contains("o.o")));
    }

    #[test]
    fn room_number_joins_on_this_computer_without_a_visible_address() {
        let host = Link::host_spec("Ada", "127.0.0.1:0", Some(Some(914))).unwrap();
        assert_eq!(host.room_number(), 914);
        let address = locate_room(914).unwrap();
        assert!(address.starts_with("127.0.0.1:"), "{address}");
        let guest = Link::join_room("Bea", &address, 914).unwrap();
        let (host_room, guest_room) = wait_rooms(&host, &guest, |host_room, guest_room| {
            host_room.number == 914
                && guest_room.number == 914
                && host_room.peers.iter().any(|name| name == "Bea")
        });
        assert!(host_room.connected() && guest_room.connected());
        let near = parse_beacon(
            b"PITTY1\t9\tAda\t914",
            "192.168.0.103:47871".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(near.number, 914);
        assert_eq!(format!("{}  {}", near.number, near.name), "914  Ada");
        assert!(parse_beacon(b"PITTY1\t9\tAda", "127.0.0.1:47871".parse().unwrap()).is_none());
        drop(guest);
        drop(host);
        assert!(read_local_room(914).is_none());
    }

    #[test]
    fn same_name_on_one_computer_stays_distinct() {
        let host = Link::host("You", "127.0.0.1:0").unwrap();
        let guest = Link::join("You", host.share()).unwrap();
        let (host_room, guest_room) = wait_rooms(&host, &guest, |host_room, guest_room| {
            host_room.peers == ["You", "You (2)"] && guest_room.peers == ["You", "You (2)"]
        });
        assert!(
            host_room
                .says
                .iter()
                .any(|say| say.text == "You (2) joined")
        );
        assert!(
            guest_room
                .says
                .iter()
                .any(|say| say.text == "You (2) joined")
        );
        guest.say("from the second pitty").unwrap();
        host.say("from the first pitty").unwrap();
        let (host_room, guest_room) = wait_rooms(&host, &guest, |host_room, guest_room| {
            said_by(host_room, "You (2)", "from the second pitty")
                && said_by(guest_room, "You (2)", "from the second pitty")
                && said_by(host_room, "You", "from the first pitty")
                && said_by(guest_room, "You", "from the first pitty")
        });
        assert!(host_room.connected() && guest_room.connected());
    }

    fn said_by(room: &Room, name: &str, text: &str) -> bool {
        room.says
            .iter()
            .any(|say| say.by == name && say.text == text)
    }

    #[test]
    fn loopback_room_shares_a_say_and_a_cat() {
        let host = Link::host("Ada", "127.0.0.1:0").unwrap();
        let guest = Link::join("Bea", host.share()).unwrap();
        let (host_room, guest_room) = wait_rooms(&host, &guest, |host_room, guest_room| {
            host_room.peers.iter().any(|name| name == "Bea")
                && guest_room.peers.iter().any(|name| name == "Ada")
                && guest_room.peers.iter().any(|name| name == "Bea")
        });
        assert!(host_room.connected() && guest_room.connected());
        guest.say("hello from bea").unwrap();
        host.say("hi\u{1b}[31m from ada").unwrap();
        let (host_room, guest_room) = wait_rooms(&host, &guest, |host_room, guest_room| {
            has_say(host_room, "hello from bea")
                && has_say(guest_room, "hello from bea")
                && has_say(host_room, "hi from ada")
                && has_say(guest_room, "hi from ada")
        });
        assert!(
            host_room
                .says
                .iter()
                .all(|say| !say.text.contains('\u{1b}'))
        );
        assert!(
            guest_room
                .says
                .iter()
                .all(|say| !say.text.contains('\u{1b}'))
        );
        guest.place("cat", builtin("cat").unwrap()).unwrap();
        let (host_room, _) = wait_rooms(&host, &guest, |host_room, guest_room| {
            art_named(host_room, "cat") && art_named(guest_room, "cat")
        });
        let id = host_room
            .arts
            .iter()
            .find(|art| art.name == "cat")
            .unwrap()
            .id;
        let x = host_room.arts.iter().find(|art| art.id == id).unwrap().x;
        host.nudge(id, 1, 0).unwrap();
        let (host_room, guest_room) = wait_rooms(&host, &guest, |host_room, guest_room| {
            art_at(host_room, id, x + 1) && art_at(guest_room, id, x + 1)
        });
        assert_eq!(
            host_room
                .arts
                .iter()
                .find(|art| art.id == id)
                .map(|art| (art.x, art.y)),
            guest_room
                .arts
                .iter()
                .find(|art| art.id == id)
                .map(|art| (art.x, art.y))
        );
    }

    fn has_say(room: &Room, text: &str) -> bool {
        room.says.iter().any(|say| say.text == text)
    }

    fn art_named(room: &Room, name: &str) -> bool {
        room.arts.iter().any(|art| art.name == name)
    }

    fn art_at(room: &Room, id: u64, x: u16) -> bool {
        room.arts.iter().any(|art| art.id == id && art.x == x)
    }

    fn wait_rooms(
        host: &Link,
        guest: &Link,
        mut ready: impl FnMut(&Room, &Room) -> bool,
    ) -> (Room, Room) {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut last = (None, None);
        while Instant::now() < deadline {
            if let Some(room) = host.poll().room {
                last.0 = Some(room);
            }
            if let Some(room) = guest.poll().room {
                last.1 = Some(room);
            }
            if let (Some(host_room), Some(guest_room)) = (&last.0, &last.1)
                && ready(host_room, guest_room)
            {
                return (host_room.clone(), guest_room.clone());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("rooms did not match in 3s: {last:?}");
    }
}
