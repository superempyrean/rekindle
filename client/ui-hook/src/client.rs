//! Reading the client's own state, on its UI thread.
//!
//! rekindle keeps no copy of the client's state: a plugin's query reads the
//! client's memory through the recovered layout. The client changes that
//! memory only on its UI thread, and the plugins run on rekindle's worker,
//! so a read is a job the worker hands to the UI thread ([`read`]). The UI
//! thread runs the jobs it was handed each frame, after the client's draw
//! and outside it ([`service`]), and keeps running new ones for a few
//! milliseconds while the worker is still busy ([`Busy`]), so a handler that
//! makes many queries waits for one frame, not one per query.
//!
//! The client's model is found the way the client hands it to its own
//! providers: rekindle's providers note the `&ChatModel` the client passes
//! them ([`saw_model`]). Until the client has called one, there is no client
//! to read and every read returns `None`.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rekindle_native::mem;

use crate::VecLayout;

/// Where the client keeps what the queries read. All from the recovered
/// layout.
#[derive(Clone, Debug, Default)]
pub struct ClientLayout {
    pub vec: VecLayout,
    /// `ChatModel`: its `FakeData`, the open server (`Option<u64>`) and
    /// channel.
    pub model_data: usize,
    pub model_guild: usize,
    pub model_channel: usize,
    /// `FakeData`: its users and servers (`Vec`s).
    pub data_users: usize,
    pub data_guilds: usize,
    pub user_size: usize,
    pub user_id: usize,
    pub user_username: usize,
    pub user_display: usize,
    pub user_bot: usize,
    pub guild_size: usize,
    pub guild_id: usize,
    pub guild_name: usize,
    pub guild_owner: usize,
    pub guild_roles: usize,
    pub guild_members: usize,
    pub role_size: usize,
    pub role_id: usize,
    pub role_name: usize,
    pub role_color: usize,
    pub role_hoist: usize,
    pub role_permissions: usize,
    pub member_size: usize,
    pub member_user: usize,
    pub member_roles: usize,
    pub member_nick: usize,
    /// `Message`.
    pub message_id: usize,
    pub message_author: usize,
    pub message_body: usize,
    pub message_edited: usize,
    pub message_pinned: usize,
}

static LAYOUT: OnceLock<ClientLayout> = OnceLock::new();
/// The client's `ChatModel`, as it last handed it to one of rekindle's
/// providers (0: not yet).
static MODEL: AtomicUsize = AtomicUsize::new(0);

/// Set the layout the reads use. The first call wins.
pub fn configure(layout: ClientLayout) {
    let _ = LAYOUT.set(layout);
}

/// Called by rekindle's providers with the `&ChatModel` the client passed
/// them. The model lives as long as the client's view, which is the whole
/// run.
pub(crate) fn saw_model(model: *const u8) {
    if !model.is_null() {
        MODEL.store(model.addr(), Ordering::SeqCst);
    }
}

/// Whether the client has handed rekindle its model, so reads can work.
pub fn ready() -> bool {
    MODEL.load(Ordering::SeqCst) != 0 && LAYOUT.get().is_some()
}

// ---- the jobs ----

type Job = Box<dyn FnOnce() + Send>;

static JOBS: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());
static JOBS_CV: Condvar = Condvar::new();
/// How many stretches of work the worker is in ([`Busy`]).
static BUSY: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// This thread is the client's UI thread, running [`service`]: a read
    /// made here runs at once.
    static ON_UI: Cell<bool> = const { Cell::new(false) };
}

/// How long a read waits for the UI thread before it gives up. The client
/// draws about every 16 ms while rekindle runs, so only a stalled client
/// makes a read wait this long.
const READ_TIMEOUT: Duration = Duration::from_millis(500);

/// How long [`service`] keeps waiting for the worker's next read while the
/// worker is busy.
pub const SERVICE_BUDGET: Duration = Duration::from_millis(4);

/// A stretch of work on rekindle's worker that may read the client. While
/// one is alive, the UI thread waits a little for its reads after running
/// those already handed over, instead of leaving them for the next frame.
pub struct Busy(());

impl Busy {
    pub fn begin() -> Self {
        BUSY.fetch_add(1, Ordering::SeqCst);
        Busy(())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.fetch_sub(1, Ordering::SeqCst);
        JOBS_CV.notify_all();
    }
}

/// Run `f` against the client, on its UI thread, and return what it
/// returned: at once if this is the UI thread, else by handing it over and
/// waiting. `None` if there is no client to read yet, or the UI thread
/// didn't get to it within [`READ_TIMEOUT`].
pub fn read<R: Send + 'static>(f: impl FnOnce(&Client) -> R + Send + 'static) -> Option<R> {
    if ON_UI.get() {
        return Client::now().map(|c| f(&c));
    }
    if !ready() {
        return None;
    }
    let answer: Arc<(Mutex<Option<Option<R>>>, Condvar)> =
        Arc::new((Mutex::new(None), Condvar::new()));
    let a = answer.clone();
    JOBS.lock().unwrap().push_back(Box::new(move || {
        let r = Client::now().map(|c| f(&c));
        *a.0.lock().unwrap() = Some(r);
        a.1.notify_all();
    }));
    JOBS_CV.notify_all();
    let slot = answer.0.lock().unwrap();
    let (mut slot, _) = answer
        .1
        .wait_timeout_while(slot, READ_TIMEOUT, |s| s.is_none())
        .unwrap();
    slot.take().flatten()
}

/// UI thread: run the reads the worker handed over, and while the worker is
/// still busy, wait up to `budget` for more.
pub fn service(budget: Duration) {
    ON_UI.set(true);
    let deadline = Instant::now() + budget;
    loop {
        let job = JOBS.lock().unwrap().pop_front();
        if let Some(job) = job {
            // A panicking read only fails that read: its waiter times out.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
            continue;
        }
        let now = Instant::now();
        if BUSY.load(Ordering::SeqCst) == 0 || now >= deadline {
            break;
        }
        let jobs = JOBS.lock().unwrap();
        let _ = JOBS_CV
            .wait_timeout_while(jobs, deadline - now, |j| {
                j.is_empty() && BUSY.load(Ordering::SeqCst) > 0
            })
            .unwrap();
    }
    ON_UI.set(false);
}

// ---- what a read returns ----

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct UserRead {
    pub id: u64,
    pub username: String,
    pub display: String,
    pub bot: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildRead {
    pub id: u64,
    pub name: String,
    pub owner: u64,
    pub members: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleRead {
    pub id: u64,
    pub name: String,
    /// `0xRRGGBB`; 0 is none.
    pub color: u32,
    pub hoist: bool,
    pub permissions: u64,
    /// Discord's position: higher is above. `@everyone` is 0.
    pub position: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemberRead {
    pub user: u64,
    pub roles: Vec<u64>,
    pub nick: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MessageRead {
    pub id: u64,
    pub author: u64,
    pub body: String,
    pub edited: bool,
    pub pinned: bool,
}

// ---- reading ----

/// The client's model, readable: only on the UI thread, inside [`read`] or
/// a provider call.
pub struct Client {
    model: usize,
    l: &'static ClientLayout,
}

/// A `T` of the client's at `at`, if it's readable.
fn word<T: Copy>(at: usize) -> Option<T> {
    // SAFETY: checked readable first; the client's memory doesn't change
    // under a read, which runs on its UI thread.
    mem::readable(at, size_of::<T>()).then(|| unsafe { mem::read::<T>(at) })
}

/// The longest list a read accepts: anything longer isn't the client's.
const MAX_LEN: usize = 100_000;

impl Client {
    fn now() -> Option<Self> {
        let model = MODEL.load(Ordering::SeqCst);
        let l = LAYOUT.get()?;
        (model != 0).then_some(Self { model, l })
    }

    /// A provider's view of the model the client just passed it.
    pub(crate) fn at(model: *const u8) -> Option<Self> {
        let l = LAYOUT.get()?;
        (!model.is_null()).then(|| Self {
            model: model.addr(),
            l,
        })
    }

    /// The entries of the client's `Vec` at `at`, each `stride` bytes.
    fn entries(&self, at: usize, stride: usize) -> Vec<usize> {
        let v = &self.l.vec;
        let (Some(cap), Some(ptr), Some(len)) = (
            word::<usize>(at + v.cap_off),
            word::<usize>(at + v.ptr_off),
            word::<usize>(at + v.len_off),
        ) else {
            return Vec::new();
        };
        if len == 0 || len > cap || len > MAX_LEN || ptr == 0 || !mem::readable(ptr, len * stride) {
            return Vec::new();
        }
        (0..len).map(|i| ptr + i * stride).collect()
    }

    /// A `&str` at `at` (pointer, then length).
    fn str_at(at: usize) -> Option<String> {
        let [ptr, len] = word::<[usize; 2]>(at)?;
        if len == 0 {
            return Some(String::new());
        }
        if ptr == 0 || len > 1 << 20 || !mem::readable(ptr, len) {
            return None;
        }
        // SAFETY: checked readable just above.
        let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
        std::str::from_utf8(bytes).ok().map(str::to_owned)
    }

    /// A `String` at `at` (a `Vec<u8>`).
    fn string_at(&self, at: usize) -> Option<String> {
        let v = &self.l.vec;
        let ptr = word::<usize>(at + v.ptr_off)?;
        let len = word::<usize>(at + v.len_off)?;
        let cap = word::<usize>(at + v.cap_off)?;
        if len == 0 {
            return Some(String::new());
        }
        if len > cap || len > 1 << 24 || ptr == 0 || !mem::readable(ptr, len) {
            return None;
        }
        // SAFETY: checked readable just above.
        let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
        std::str::from_utf8(bytes).ok().map(str::to_owned)
    }

    /// An `Option<&str>` at `at`: `None` is a null pointer.
    fn opt_str_at(at: usize) -> Option<String> {
        let ptr = word::<usize>(at)?;
        if ptr == 0 { None } else { Self::str_at(at) }
    }

    fn data(&self) -> usize {
        self.model + self.l.model_data
    }

    /// The open server, and channel.
    pub fn selection(&self) -> (Option<u64>, u64) {
        let guild = word::<Option<u64>>(self.model + self.l.model_guild).flatten();
        let channel = word::<u64>(self.model + self.l.model_channel).unwrap_or(0);
        (guild, channel)
    }

    pub fn users(&self) -> Vec<UserRead> {
        let l = self.l;
        self.entries(self.data() + l.data_users, l.user_size)
            .into_iter()
            .filter_map(|u| {
                Some(UserRead {
                    id: word(u + l.user_id)?,
                    username: Self::str_at(u + l.user_username)?,
                    display: Self::str_at(u + l.user_display)?,
                    bot: word::<u8>(u + l.user_bot)? != 0,
                })
            })
            .collect()
    }

    pub fn user(&self, id: u64) -> Option<UserRead> {
        self.users().into_iter().find(|u| u.id == id)
    }

    fn guild_entries(&self) -> Vec<usize> {
        self.entries(self.data() + self.l.data_guilds, self.l.guild_size)
    }

    fn guild_at(&self, id: u64) -> Option<usize> {
        self.guild_entries()
            .into_iter()
            .find(|g| word::<u64>(g + self.l.guild_id) == Some(id))
    }

    pub fn guild_ids(&self) -> Vec<u64> {
        self.guild_entries()
            .into_iter()
            .filter_map(|g| word(g + self.l.guild_id))
            .collect()
    }

    pub fn guild(&self, id: u64) -> Option<GuildRead> {
        let l = self.l;
        let g = self.guild_at(id)?;
        Some(GuildRead {
            id,
            name: Self::str_at(g + l.guild_name)?,
            owner: word(g + l.guild_owner)?,
            members: self.entries(g + l.guild_members, l.member_size).len() as u32,
        })
    }

    /// A server's roles, highest first, as the client keeps them.
    pub fn roles(&self, guild: u64) -> Vec<RoleRead> {
        let l = self.l;
        let Some(g) = self.guild_at(guild) else {
            return Vec::new();
        };
        let entries = self.entries(g + l.guild_roles, l.role_size);
        let n = entries.len() as i32;
        entries
            .into_iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let id: u64 = word(r + l.role_id)?;
                Some(RoleRead {
                    id,
                    name: Self::str_at(r + l.role_name)?,
                    color: word(r + l.role_color)?,
                    hoist: word::<u8>(r + l.role_hoist)? != 0,
                    permissions: word(r + l.role_permissions)?,
                    // `@everyone` (the server's own id) is position 0.
                    position: if id == guild { 0 } else { n - 1 - i as i32 },
                })
            })
            .collect()
    }

    pub fn members(&self, guild: u64) -> Vec<MemberRead> {
        let l = self.l;
        let Some(g) = self.guild_at(guild) else {
            return Vec::new();
        };
        self.entries(g + l.guild_members, l.member_size)
            .into_iter()
            .filter_map(|m| {
                Some(MemberRead {
                    user: word(m + l.member_user)?,
                    roles: self
                        .entries(m + l.member_roles, 8)
                        .into_iter()
                        .filter_map(word)
                        .collect(),
                    nick: Self::opt_str_at(m + l.member_nick),
                })
            })
            .collect()
    }

    pub fn member(&self, guild: u64, user: u64) -> Option<MemberRead> {
        self.members(guild).into_iter().find(|m| m.user == user)
    }

    /// The client's `Message` at `m`, as a provider was handed it.
    pub fn message_at(&self, m: *const u8) -> Option<MessageRead> {
        let l = self.l;
        let m = m.addr();
        Some(MessageRead {
            id: word(m + l.message_id)?,
            author: word(m + l.message_author)?,
            body: self.string_at(m + l.message_body)?,
            edited: word::<u8>(m + l.message_edited)? != 0,
            pinned: word::<u8>(m + l.message_pinned)? != 0,
        })
    }
}
