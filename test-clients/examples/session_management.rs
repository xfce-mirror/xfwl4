// xfwl4 -- Wayland compositor for the Xfce Desktop Environment
//
// Copyright (C) 2026 Brian Tarricone <brian@tarricone.org>
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Test client for the xdg-session-management-v1 protocol.
//!
//! Exercises the session lifecycle standalone (no session manager required):
//! creating/adding, restoring, renaming, and removing.  A typical exercise
//! sequence:
//!
//! ```text
//! session-management add          # map a window, add it, cache the session id
//! # move/resize the window, then Ctrl-C to exit
//! session-management restore      # relaunch: window should come back as it was
//! session-management rename win1 newname
//! session-management remove win1  # drop the toplevel from the stored session
//! session-management remove-session
//! ```
//!
//! The session id from the most recent `add` (or `restore`, if it was a new
//! session) is cached in a state file so subsequent commands find it
//! automatically; pass `--id` to override.
//!
//! Protocol errors (name in use, invalid name, already mapped, etc.) kill the
//! connection; the resulting dispatch error is printed and the client exits.

use std::{path::PathBuf, time::Duration};

use clap::{Parser, Subcommand, ValueEnum};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_dispatch2, delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{LoopHandle, LoopSignal, timer::TimeoutAction, timer::Timer},
        client::{
            Connection, Proxy, QueueHandle,
            globals::GlobalList,
            protocol::{
                wl_output::{Transform, WlOutput},
                wl_surface::WlSurface,
            },
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use test_clients::wayland::{apply_window_configure, init_event_loop, paint_solid};

mod proto {
    use smithay_client_toolkit::reexports::{client as wayland_client, protocols::xdg::shell::client::xdg_toplevel};

    pub mod __interfaces {
        use smithay_client_toolkit::reexports::{client::backend as wayland_backend, protocols::xdg::shell::client::__interfaces::*};

        wayland_scanner::generate_interfaces!("../resources/xdg-session-management-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("../resources/xdg-session-management-v1.xml");
}

use proto::{
    xdg_session_manager_v1::{Reason, XdgSessionManagerV1},
    xdg_session_v1::XdgSessionV1,
    xdg_toplevel_session_v1::XdgToplevelSessionV1,
};

const DEFAULT_SIZE: (u32, u32) = (400, 300);
const DEFAULT_NAME: &str = "win1";

fn default_session_id_path() -> PathBuf {
    std::env::temp_dir().join("xfwl4-session-management-test-session-id")
}

#[derive(Clone, Parser)]
#[command(name = "session-management", about = "Test client for the xdg-session-management-v1 protocol")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// session id to use (default: cached from the last add/restore)
    #[arg(long)]
    id: Option<String>,

    /// reason for the session request (default depends on the command)
    #[arg(long)]
    reason: Option<ReasonArg>,
}

#[derive(Clone, PartialEq, Eq, Debug, Subcommand)]
enum Command {
    /// map a window and add it to a new (or restored) session
    Add {
        /// name identifying the toplevel in the session
        name: Option<String>,
    },
    /// map a window and restore its saved state before first commit
    Restore {
        /// name identifying the toplevel in the session
        name: Option<String>,
    },
    /// map a window, restore/add it as OLD, then rename to NEW
    Rename {
        /// the toplevel's existing name in the session
        old: String,
        /// the new name for the toplevel
        new: String,
    },
    /// remove a toplevel from the stored session (no window)
    Remove {
        /// name identifying the toplevel in the session
        name: String,
    },
    /// remove the stored session entirely (no window)
    RemoveSession,
}

impl Command {
    fn wants_window(&self) -> bool {
        !matches!(self, Command::Remove { .. } | Command::RemoveSession)
    }

    fn default_reason(&self) -> Reason {
        match self {
            Command::Add { .. } => Reason::Launch,
            Command::Restore { .. } | Command::Rename { .. } => Reason::Recover,
            Command::Remove { .. } | Command::RemoveSession => Reason::Launch,
        }
    }

    fn names(&self, default_name: &str) -> Vec<String> {
        match self {
            Command::Add { name } | Command::Restore { name } => vec![name.clone().unwrap_or_else(|| default_name.to_string())],
            Command::Rename { old, new } => vec![old.clone(), new.clone()],
            Command::Remove { name } => vec![name.clone()],
            Command::RemoveSession => Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
enum ReasonArg {
    Launch,
    Recover,
    #[value(name = "session-restore")]
    SessionRestore,
}

impl From<ReasonArg> for Reason {
    fn from(arg: ReasonArg) -> Reason {
        match arg {
            ReasonArg::Launch => Reason::Launch,
            ReasonArg::Recover => Reason::Recover,
            ReasonArg::SessionRestore => Reason::SessionRestore,
        }
    }
}

struct SessionTestClient {
    command: Command,
    reason: Reason,
    names: Vec<String>,
    session_id_path: PathBuf,

    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,

    loop_handle: LoopHandle<'static, Self>,
    loop_signal: LoopSignal,

    session: Option<XdgSessionV1>,
    window: Option<Window>,
    toplevel_session: Option<XdgToplevelSessionV1>,

    first_configure: bool,
    buffer: Option<Buffer>,
    width: u32,
    height: u32,
}

impl SessionTestClient {
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let Some(window) = &self.window else {
            return;
        };
        let color = match self.command {
            Command::Add { .. } => [0x44, 0xaa, 0x66, 0xff],
            Command::Restore { .. } | Command::Rename { .. } => [0xaa, 0x66, 0x22, 0xff],
            Command::Remove { .. } | Command::RemoveSession => [0x88, 0x88, 0x88, 0xff],
        };
        paint_solid(
            &mut self.pool,
            &mut self.buffer,
            window.wl_surface(),
            qh,
            self.width,
            self.height,
            color,
        );
        window.commit();
    }

    fn exit_soon(&self, delay: Duration) {
        let signal = self.loop_signal.clone();
        self.loop_handle
            .insert_source(Timer::from_duration(delay), move |_, _, _| {
                signal.stop();
                TimeoutAction::Drop
            })
            .unwrap();
    }

    fn describe_request(&self, session_id: Option<&str>) {
        let reason_str = match self.reason {
            Reason::Launch => "launch",
            Reason::Recover => "recover",
            Reason::SessionRestore => "session-restore",
        };
        match session_id {
            Some(id) => eprintln!("requesting session '{id}' (reason {reason_str})"),
            None => eprintln!("requesting a new session (reason {reason_str})"),
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let command = cli.command;

    let session_id_path = default_session_id_path();
    let mut session_id = cli.id.clone();
    if session_id.is_none() {
        session_id = std::fs::read_to_string(&session_id_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    let reason = cli.reason.map(Reason::from).unwrap_or_else(|| command.default_reason());
    let names = command.names(DEFAULT_NAME);

    let (conn, globals, qh, mut event_loop) = init_event_loop::<SessionTestClient>();

    let session_manager = bind_session_manager(&globals, &qh).unwrap_or_else(|err| {
        eprintln!("{err}");
        std::process::exit(1);
    });

    let (shm, pool, window) = if command.wants_window() {
        let compositor = CompositorState::bind(&globals, &qh).unwrap();
        let xdg_shell = XdgShell::bind(&globals, &qh).unwrap();
        let shm = Shm::bind(&globals, &qh).unwrap();
        let pool = SlotPool::new(DEFAULT_SIZE.0 as usize * DEFAULT_SIZE.1 as usize * 4, &shm).unwrap();

        let surface = compositor.create_surface(&qh);
        let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
        window.set_title(&format!("session-management: {}", names[0]));
        window.set_app_id("org.xfce.xfwl4.session-management-test");
        (shm, pool, Some(window))
    } else {
        let shm = Shm::bind(&globals, &qh).unwrap();
        let pool = SlotPool::new(64, &shm).unwrap();
        (shm, pool, None)
    };

    let mut state = SessionTestClient {
        command,
        reason,
        names,
        session_id_path,

        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,

        loop_handle: event_loop.handle(),
        loop_signal: event_loop.get_signal(),

        session: None,
        window,
        toplevel_session: None,

        first_configure: true,
        buffer: None,
        width: DEFAULT_SIZE.0,
        height: DEFAULT_SIZE.1,
    };

    state.describe_request(session_id.as_deref());
    let session = session_manager.get_session(reason, session_id.clone(), &qh, ());
    state.session = Some(session);

    match state.command {
        Command::Add { .. } => {
            // add_toplevel is sent once the window is mapped (first configure),
            // like a real application that adopts its window post-mapping. The
            // initial commit has to happen here, though.
            state.window.as_ref().unwrap().commit();
        }
        Command::Restore { .. } | Command::Rename { .. } => {
            // restore_toplevel must precede the window's first commit.
            let window = state.window.as_ref().unwrap();
            let toplevel = window.xdg_toplevel().clone();
            let name = state.names[0].clone();
            let tls = state.session.as_ref().unwrap().restore_toplevel(&toplevel, name.clone(), &qh, ());
            state.toplevel_session = Some(tls);
            eprintln!("sent restore_toplevel('{name}') before first commit");
            window.commit();
        }
        Command::Remove { .. } => {
            let name = state.names[0].clone();
            state.session.as_ref().unwrap().remove_toplevel(name.clone());
            eprintln!("sent remove_toplevel('{name}'); exiting");
            state.exit_soon(Duration::from_millis(300));
        }
        Command::RemoveSession => {
            state.session.as_ref().unwrap().remove();
            eprintln!("sent session remove(); exiting");
            state.exit_soon(Duration::from_millis(300));
        }
    }

    if state.command.wants_window() {
        eprintln!("running; Ctrl-C to exit");
    }
    event_loop
        .run(Duration::from_millis(16), &mut state, |_state| {
            // Protocol errors (e.g. name_in_use) kill the connection, but calloop-wayland-source
            // drops errors raised during the socket read, so the loop would otherwise spin on a
            // dead connection forever. Check for it explicitly and exit.
            if let Some(err) = conn.protocol_error() {
                eprintln!("wayland connection error: {err}");
                std::process::exit(1);
            }
        })
        .unwrap();
}

fn bind_session_manager(globals: &GlobalList, qh: &QueueHandle<SessionTestClient>) -> Result<XdgSessionManagerV1, String> {
    globals
        .bind(qh, 1..=1, ())
        .map_err(|_| "compositor does not advertise xdg_session_manager_v1 (is xfwl4 running with session management?)".to_string())
}

impl Dispatch2<XdgSessionManagerV1, SessionTestClient> for () {
    fn event(
        &self,
        _state: &mut SessionTestClient,
        _manager: &XdgSessionManagerV1,
        event: <XdgSessionManagerV1 as Proxy>::Event,
        _conn: &Connection,
        _qh: &QueueHandle<SessionTestClient>,
    ) {
        match event {}
    }
}

impl Dispatch2<XdgSessionV1, SessionTestClient> for () {
    fn event(
        &self,
        state: &mut SessionTestClient,
        _session: &XdgSessionV1,
        event: <XdgSessionV1 as Proxy>::Event,
        _conn: &Connection,
        _qh: &QueueHandle<SessionTestClient>,
    ) {
        match event {
            proto::xdg_session_v1::Event::Created { session_id } => {
                eprintln!("session created with id '{session_id}'");
                if let Err(err) = std::fs::write(&state.session_id_path, &session_id) {
                    eprintln!("failed to cache session id: {err}");
                } else {
                    eprintln!("cached session id at {}", state.session_id_path.display());
                }
            }
            proto::xdg_session_v1::Event::Restored => {
                eprintln!("session restored (existing state found)");
            }
            proto::xdg_session_v1::Event::Replaced => {
                eprintln!("session was replaced by another client; its objects are now inert");
            }
        }
    }
}

impl Dispatch2<XdgToplevelSessionV1, SessionTestClient> for () {
    fn event(
        &self,
        state: &mut SessionTestClient,
        _tls: &XdgToplevelSessionV1,
        event: <XdgToplevelSessionV1 as Proxy>::Event,
        _conn: &Connection,
        _qh: &QueueHandle<SessionTestClient>,
    ) {
        match event {
            proto::xdg_toplevel_session_v1::Event::Restored => {
                eprintln!(
                    "toplevel '{}' restored: saved state is being applied with this configure",
                    state.names[0]
                );
            }
        }
    }
}

impl ProvidesRegistryState for SessionTestClient {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

impl CompositorHandler for SessionTestClient {
    fn frame(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _time: u32) {
        // The content is static; drawing here would commit, which schedules the next
        // frame callback, which draws again — a continuous redraw loop. All drawing
        // happens in response to configures instead.
    }

    fn surface_enter(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _output: &WlOutput) {}

    fn surface_leave(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _output: &WlOutput) {}

    fn transform_changed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _new_transform: Transform) {}

    fn scale_factor_changed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _surface: &WlSurface, _new_factor: i32) {}
}

impl OutputHandler for SessionTestClient {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}

    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}
}

impl ShmHandler for SessionTestClient {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl WindowHandler for SessionTestClient {
    fn configure(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, _window: &Window, configure: WindowConfigure, _serial: u32) {
        let (new_w, new_h, redraw) = apply_window_configure(&configure, self.first_configure, (self.width, self.height), DEFAULT_SIZE);
        if redraw {
            let first_configure = self.first_configure;
            self.first_configure = false;
            self.buffer = None;
            self.width = new_w;
            self.height = new_h;
            self.draw(qh);

            // Protocol requests happen on the first configure only; later configures are
            // just resizes, and re-sending add_toplevel/rename is a protocol error.
            if first_configure {
                match self.command {
                    Command::Add { .. } => {
                        let name = self.names[0].clone();
                        if let (Some(session), Some(window)) = (&self.session, &self.window) {
                            let tls = session.add_toplevel(window.xdg_toplevel(), name.clone(), qh, ());
                            self.toplevel_session = Some(tls);
                            eprintln!("sent add_toplevel('{name}')");
                        }
                    }
                    Command::Rename { .. } => {
                        let (old, new) = (self.names[0].clone(), self.names[1].clone());
                        if let Some(tls) = &self.toplevel_session {
                            tls.rename(new.clone());
                            eprintln!("sent rename('{old}' -> '{new}')");
                        }
                    }
                    Command::Restore { .. } | Command::Remove { .. } | Command::RemoveSession => {}
                }
            }
        } else if let Some(window) = &self.window {
            window.commit();
        }
    }

    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &Window) {
        eprintln!("close requested; exiting");
        self.loop_signal.stop();
    }
}

delegate_registry!(SessionTestClient);
delegate_dispatch2!(SessionTestClient);
