use embassy_sync::{
    blocking_mutex::raw::NoopRawMutex,
    mutex::{Mutex, MutexGuard},
};
use static_cell::StaticCell;
use zenoh_proto::{Endpoint, TransportLinkError};

use crate::{
    api::callbacks::ZCallbacks,
    config::ZSessionConfig,
    io::{driver::Driver, transport::TransportLink},
    platform::ZLinkManager,
    resources::Resources,
};

mod run;

pub mod get;
pub mod r#pub;
pub mod put;
pub mod querier;
pub mod queryable;
pub mod sub;

pub(crate) struct SessionState<'res, Config>
where
    Config: ZSessionConfig + 'res,
{
    next: u32,
    sub_callbacks: Config::SubCallbacks<'res>,
    get_callbacks: Config::GetCallbacks<'res>,
    queryable_callbacks: Config::QueryableCallbacks<'res>,
}

impl<'res, Config> SessionState<'res, Config>
where
    Config: ZSessionConfig,
{
    pub fn new() -> Self {
        Self {
            next: 0,
            sub_callbacks: Config::SubCallbacks::empty(),
            get_callbacks: Config::GetCallbacks::empty(),
            queryable_callbacks: Config::QueryableCallbacks::empty(),
        }
    }

    pub(crate) fn next(&mut self) -> u32 {
        let next = self.next;
        self.next += 1;
        next
    }
}

pub struct Session<'res, Config>
where
    Config: ZSessionConfig,
{
    driver: Driver<'res, <Config::LinkManager as ZLinkManager>::Link<'res>, Config::Buff>,
    state: Mutex<NoopRawMutex, SessionState<'res, Config>>,
}

impl<'res, Config> Session<'res, Config>
where
    Config: ZSessionConfig,
{
    pub fn new(
        transport: &'res mut TransportLink<
            <Config::LinkManager as ZLinkManager>::Link<'res>,
            Config::Buff,
        >,
    ) -> Self {
        Self {
            driver: Driver::new(transport),
            state: Mutex::new(SessionState::new()),
        }
    }

    pub(crate) async fn state(&self) -> MutexGuard<'_, NoopRawMutex, SessionState<'res, Config>> {
        self.state.lock().await
    }
}

/// Prepares a session using caller-provided resources and configuration.
///
/// Options are applied after the transport handshake and before the session's
/// driver is constructed. Each builder is consumed by `connect` or `listen`.
#[must_use = "a session builder must be connected or listened on"]
pub struct SessionBuilder<'res, Config: ZSessionConfig> {
    #[doc(hidden)]
    pub resources: &'res mut Resources<'res, Config>,
    #[doc(hidden)]
    pub config: &'res Config,
    #[doc(hidden)]
    pub ignore_invalid_sn: bool,
}

impl<'res, Config: ZSessionConfig> SessionBuilder<'res, Config> {
    pub fn new(resources: &'res mut Resources<'res, Config>, config: &'res Config) -> Self {
        Self {
            resources,
            config,
            ignore_invalid_sn: false,
        }
    }

    /// Disables validation of received transport frame sequence numbers.
    pub fn ignore_invalid_sn(mut self) -> Self {
        self.ignore_invalid_sn = true;
        self
    }

    /// Establishes an outgoing connection and creates the session.
    pub async fn connect(
        self,
        endpoint: Endpoint<'_>,
    ) -> core::result::Result<Session<'res, Config>, TransportLinkError> {
        let transport = self
            .config
            .transports()
            .connect(endpoint, self.config.buff())
            .await?;
        Ok(self.finish(transport))
    }

    /// Accepts an incoming connection and creates the session.
    pub async fn listen(
        self,
        endpoint: Endpoint<'_>,
    ) -> core::result::Result<Session<'res, Config>, TransportLinkError> {
        let transport = self
            .config
            .transports()
            .listen(endpoint, self.config.buff())
            .await?;
        Ok(self.finish(transport))
    }

    fn finish(
        self,
        mut transport: TransportLink<
            <Config::LinkManager as ZLinkManager>::Link<'res>,
            Config::Buff,
        >,
    ) -> Session<'res, Config> {
        if self.ignore_invalid_sn {
            transport.transport_mut().rx.ignore_invalid_sn();
        }
        Session::new(self.resources.init(transport))
    }
}

/// Prepares a session whose configuration, resources, and session have static storage.
///
/// Usually created with `zenoh::session!`. Each builder is consumed by `connect`
/// or `listen`, which returns a shared reference to the initialized session.
/// Storage cannot be reused, including after a connection error or cancellation.
#[must_use = "a session builder must be connected or listened on"]
pub struct StaticSessionBuilder<Config: ZSessionConfig + 'static> {
    builder: SessionBuilder<'static, Config>,
    session: &'static StaticCell<Session<'static, Config>>,
}

impl<Config: ZSessionConfig + 'static> StaticSessionBuilder<Config> {
    /// Prepares a session using static resources, configuration, and an empty session cell.
    pub fn new(
        resources: &'static mut Resources<'static, Config>,
        config: &'static Config,
        session: &'static StaticCell<Session<'static, Config>>,
    ) -> Self {
        Self {
            builder: SessionBuilder::new(resources, config),
            session,
        }
    }

    /// Disables validation of received transport frame sequence numbers.
    pub fn ignore_invalid_sn(mut self) -> Self {
        self.builder = self.builder.ignore_invalid_sn();
        self
    }

    /// Establishes an outgoing connection and stores the session for the program's lifetime.
    pub async fn connect(
        self,
        endpoint: Endpoint<'_>,
    ) -> core::result::Result<&'static Session<'static, Config>, TransportLinkError> {
        let session = self.builder.connect(endpoint).await?;
        Ok(self.session.init(session))
    }

    /// Accepts an incoming connection and stores the session for the program's lifetime.
    pub async fn listen(
        self,
        endpoint: Endpoint<'_>,
    ) -> core::result::Result<&'static Session<'static, Config>, TransportLinkError> {
        let session = self.builder.listen(endpoint).await?;
        Ok(self.session.init(session))
    }
}

#[macro_export]
macro_rules! __static_session {
    ($CONFIG:ty: $config:expr $(,)?) => {{
        static CONFIG: $crate::session::__StaticCell<$CONFIG> =
            $crate::session::__StaticCell::new();
        static RESOURCES: $crate::session::__StaticCell<
            $crate::session::Resources<'static, $CONFIG>,
        > = $crate::session::__StaticCell::new();
        static SESSION: $crate::session::__StaticCell<$crate::session::Session<'static, $CONFIG>> =
            $crate::session::__StaticCell::new();

        $crate::session::StaticSessionBuilder::new(
            RESOURCES.init_with($crate::session::Resources::default),
            CONFIG.init($config),
            &SESSION,
        )
    }};
}

/// Prepares a session builder.
///
/// The expression form uses local resources. Use the `static` form when the
/// session needs static storage. Static storage can be initialized only once per call site,
/// including when opening a session fails or is cancelled.
#[macro_export]
macro_rules! __session {
    (static $CONFIG:ty: $config:expr $(,)?) => {
        $crate::__static_session!($CONFIG: $config)
    };
    ($CONFIG:ty: $config:expr $(,)?) => {{
        $crate::session::SessionBuilder::<$CONFIG> {
            resources: &mut $crate::session::Resources::default(),
            config: &$config,
            ignore_invalid_sn: false,
        }
    }};
}

impl<'res, Config: ZSessionConfig> Session<'res, Config> {
    /// Drain adapter-owned TX buffers before closing a finite application.
    /// The session driver should keep being polled while this completes.
    pub async fn flush(&self) -> Result<(), zenoh_proto::SessionError> {
        use crate::io::transport::ZTransportLinkTx;
        self.driver.tx().await.flush().await.map_err(Into::into)
    }

    /// Remove all application declarations and pending gets. Call only after
    /// stopping their owners; dropping a future alone does not undeclare it.
    pub async fn clear_declarations(&self) -> Result<(), zenoh_proto::SessionError> {
        use crate::io::transport::ZTransportLinkTx;
        use zenoh_proto::{fields::*, msgs::*};
        if self.driver.tx().await.is_interrupted() {
            return Err(zenoh_proto::TransportLinkError::TransportClosed.into());
        }
        let mut state = self.state().await;
        state.get_callbacks.clear();
        while let Some(id) = state.sub_callbacks.first_id() {
            state.sub_callbacks.remove(id)?;
            self.driver
                .tx()
                .await
                .send(core::iter::once(NetworkMessage {
                    reliability: Reliability::default(),
                    qos: zenoh_proto::exts::QoS::default(),
                    body: NetworkBody::Declare(Declare {
                        body: DeclareBody::UndeclareSubscriber(UndeclareSubscriber {
                            id,
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                }))
                .await?;
        }
        while let Some(id) = state.queryable_callbacks.first_id() {
            state.queryable_callbacks.remove(id)?;
            self.driver
                .tx()
                .await
                .send(core::iter::once(NetworkMessage {
                    reliability: Reliability::default(),
                    qos: zenoh_proto::exts::QoS::default(),
                    body: NetworkBody::Declare(Declare {
                        body: DeclareBody::UndeclareQueryable(UndeclareQueryable {
                            id,
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                }))
                .await?;
        }
        state.queryable_callbacks.clear();
        Ok(())
    }
}

/// Reusable, pinned storage for a session backed by a static configuration.
/// A guard bounds access to each session; dropping it releases the driver before
/// its transport, including on cancellation. No allocation is required.
pub struct SessionSlot<Config: ZSessionConfig + 'static> {
    session: core::mem::MaybeUninit<Session<'static, Config>>,
    transport: core::mem::MaybeUninit<
        TransportLink<<Config::LinkManager as ZLinkManager>::Link<'static>, Config::Buff>,
    >,
    occupied: bool,
    _pin: core::marker::PhantomPinned,
}
impl<Config: ZSessionConfig + 'static> Default for SessionSlot<Config> {
    fn default() -> Self {
        Self::new()
    }
}
impl<Config: ZSessionConfig + 'static> SessionSlot<Config> {
    pub const fn new() -> Self {
        Self {
            session: core::mem::MaybeUninit::uninit(),
            transport: core::mem::MaybeUninit::uninit(),
            occupied: false,
            _pin: core::marker::PhantomPinned,
        }
    }
    pub fn insert(
        self: core::pin::Pin<&mut Self>,
        transport: TransportLink<
            <Config::LinkManager as ZLinkManager>::Link<'static>,
            Config::Buff,
        >,
    ) -> SessionGuard<'_, Config> {
        // SAFETY: the slot is pinned before constructing the self-reference.
        // References returned by the guard borrow the guard, never the internal
        // static lifetime. No driver or transport can escape or be moved while
        // initialized. Config-owned link resources must themselves be static.
        unsafe {
            let slot = self.get_unchecked_mut();
            slot.clear();
            slot.transport.write(transport);
            let transport: &'static mut TransportLink<_, _> = &mut *slot.transport.as_mut_ptr();
            slot.session.write(Session::new(transport));
            slot.occupied = true;
            SessionGuard {
                slot: core::pin::Pin::new_unchecked(slot),
            }
        }
    }
    fn clear(&mut self) {
        if self.occupied {
            self.occupied = false;
            // SAFETY: both fields are initialized together. The driver is
            // destroyed before the borrowed transport. Pinning also protects
            // this order if a caller forgets a guard and later drops the slot.
            unsafe {
                self.session.assume_init_drop();
                self.transport.assume_init_drop();
            }
        }
    }
}
impl<Config: ZSessionConfig + 'static> Drop for SessionSlot<Config> {
    fn drop(&mut self) {
        self.clear();
    }
}
pub struct SessionGuard<'slot, Config: ZSessionConfig + 'static> {
    slot: core::pin::Pin<&'slot mut SessionSlot<Config>>,
}
impl<Config: ZSessionConfig + 'static> SessionGuard<'_, Config> {
    pub fn session(&self) -> &Session<'static, Config> {
        // SAFETY: insert initialized the session and the guard excludes clear.
        unsafe { self.slot.session.assume_init_ref() }
    }
}
impl<Config: ZSessionConfig + 'static> Drop for SessionGuard<'_, Config> {
    fn drop(&mut self) {
        // SAFETY: destruction does not move either pinned field.
        unsafe {
            self.slot.as_mut().get_unchecked_mut().clear();
        }
    }
}

#[cfg(test)]
mod slot_tests {
    extern crate std;
    use super::*;
    use crate::session::TransportLinkManager;
    use crate::{
        platform::*,
        session::{
            FixedCapacityGetCallbacks, FixedCapacityQueryableCallbacks, FixedCapacitySubCallbacks,
        },
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Counts {
        halves: AtomicUsize,
        links: AtomicUsize,
    }
    struct Link(Arc<Counts>);
    struct Half(Arc<Counts>);
    impl Drop for Link {
        fn drop(&mut self) {
            self.0.links.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl Drop for Half {
        fn drop(&mut self) {
            assert_eq!(self.0.links.load(Ordering::SeqCst), 0);
            self.0.halves.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl ZLinkInfo for Link {
        fn mtu(&self) -> u16 {
            64
        }
        fn is_streamed(&self) -> bool {
            true
        }
    }
    impl ZLinkInfo for Half {
        fn mtu(&self) -> u16 {
            64
        }
        fn is_streamed(&self) -> bool {
            true
        }
    }
    impl ZLinkTx for Half {
        async fn write_all(&mut self, _: &[u8]) -> Result<(), LinkError> {
            Ok(())
        }
    }
    impl ZLinkRx for Half {
        async fn read(&mut self, _: &mut [u8]) -> Result<usize, LinkError> {
            core::future::pending().await
        }
        async fn read_exact(&mut self, _: &mut [u8]) -> Result<(), LinkError> {
            core::future::pending().await
        }
    }
    impl ZLinkTx for Link {
        async fn write_all(&mut self, _: &[u8]) -> Result<(), LinkError> {
            Ok(())
        }
    }
    impl ZLinkRx for Link {
        async fn read(&mut self, _: &mut [u8]) -> Result<usize, LinkError> {
            core::future::pending().await
        }
        async fn read_exact(&mut self, _: &mut [u8]) -> Result<(), LinkError> {
            core::future::pending().await
        }
    }
    impl ZLink for Link {
        type Tx<'a> = Half;
        type Rx<'a> = Half;
        fn split(&mut self) -> (Half, Half) {
            (Half(self.0.clone()), Half(self.0.clone()))
        }
    }
    struct Manager;
    impl ZLinkManager for Manager {
        type Link<'a> = Link;
        async fn connect(&self, _: Endpoint<'_>) -> Result<Link, LinkError> {
            Err(LinkError::CouldNotConnect)
        }
        async fn listen(&self, _: Endpoint<'_>) -> Result<Link, LinkError> {
            Err(LinkError::CouldNotListen)
        }
    }
    struct Config;
    impl ZSessionConfig for Config {
        type Buff = [u8; 64];
        type LinkManager = Manager;
        type SubCallbacks<'a> = FixedCapacitySubCallbacks<'a, 2>;
        type GetCallbacks<'a> = FixedCapacityGetCallbacks<'a, 2>;
        type QueryableCallbacks<'a> = FixedCapacityQueryableCallbacks<'a, Self, 2>;
        fn transports(&self) -> &TransportLinkManager<Manager> {
            unreachable!()
        }
        fn buff(&self) -> Self::Buff {
            [0; 64]
        }
    }
    fn transport(counts: Arc<Counts>) -> TransportLink<Link, [u8; 64]> {
        TransportLink::new(
            Link(counts),
            zenoh_sansio::Transport::builder([0; 64]).codec(),
        )
    }
    #[test]
    fn slot_releases_driver_before_link_on_every_reuse() {
        let mut slot = core::pin::pin!(SessionSlot::<Config>::new());
        for _ in 0..1000 {
            let counts = Arc::new(Counts {
                halves: AtomicUsize::new(0),
                links: AtomicUsize::new(0),
            });
            let guard = slot.as_mut().insert(transport(counts.clone()));
            let _session = guard.session();
            drop(guard);
            assert_eq!(counts.halves.load(Ordering::SeqCst), 2);
            assert_eq!(counts.links.load(Ordering::SeqCst), 1);
        }
    }
    #[test]
    fn forgotten_guard_is_reclaimed_when_the_pinned_slot_drops() {
        let counts = Arc::new(Counts {
            halves: AtomicUsize::new(0),
            links: AtomicUsize::new(0),
        });
        {
            let mut slot = core::pin::pin!(SessionSlot::<Config>::new());
            let guard = slot.as_mut().insert(transport(counts.clone()));
            core::mem::forget(guard);
        }
        assert_eq!(counts.halves.load(Ordering::SeqCst), 2);
        assert_eq!(counts.links.load(Ordering::SeqCst), 1);
    }
}
