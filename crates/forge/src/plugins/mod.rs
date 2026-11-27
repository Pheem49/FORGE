//! Built-in native desktop capability plugins implementing [`crate::Plugin`].

#[cfg(feature = "plugin-clipboard")]
pub mod clipboard;
#[cfg(feature = "plugin-deep-link")]
pub mod deep_link;
#[cfg(feature = "plugin-dialog")]
pub mod dialog;
#[cfg(feature = "plugin-fs")]
pub mod fs;
#[cfg(feature = "plugin-notification")]
pub mod notification;
#[cfg(feature = "plugin-opener")]
pub mod opener;
#[cfg(feature = "plugin-shortcut")]
pub mod shortcut;
#[cfg(feature = "plugin-updater")]
pub mod updater;

#[cfg(feature = "plugin-clipboard")]
pub use clipboard::ClipboardPlugin;
#[cfg(feature = "plugin-deep-link")]
pub use deep_link::DeepLinkPlugin;
#[cfg(feature = "plugin-dialog")]
pub use dialog::DialogPlugin;
#[cfg(feature = "plugin-fs")]
pub use fs::AppFsPlugin;
#[cfg(feature = "plugin-notification")]
pub use notification::NotificationPlugin;
#[cfg(feature = "plugin-opener")]
pub use opener::{OpenerPlugin, OsUrlOpener, RecordedUrlOpener, UrlOpener};
#[cfg(feature = "plugin-shortcut")]
pub use shortcut::{GlobalShortcutPlugin, HostShortcutBackend, ShortcutBackend};
#[cfg(feature = "plugin-updater")]
pub use updater::{
    ApplyOutcome, AvailableUpdate, MemoryUpdateTransport, OsUpdateApplier, UpdateApplier,
    UpdateTransport, UpdaterPlugin, UreqUpdateTransport,
};

/// Convenience bundle registering all enabled built-in FORGE desktop plugins
/// (`DialogPlugin`, `ClipboardPlugin`, `NotificationPlugin`, `AppFsPlugin`,
/// `OpenerPlugin`, `DeepLinkPlugin`, `GlobalShortcutPlugin`, and `UpdaterPlugin`).
#[derive(Default, Clone, Copy)]
pub struct StandardPlugins;

impl crate::Plugin for StandardPlugins {
    fn apply(self, app: crate::App) -> crate::App {
        #[allow(unused_mut)]
        let mut app = app;
        #[cfg(feature = "plugin-dialog")]
        {
            app = app.plugin(DialogPlugin::new());
        }
        #[cfg(feature = "plugin-clipboard")]
        {
            app = app.plugin(ClipboardPlugin::new());
        }
        #[cfg(feature = "plugin-notification")]
        {
            app = app.plugin(NotificationPlugin::new());
        }
        #[cfg(feature = "plugin-fs")]
        {
            app = app.plugin(AppFsPlugin::new());
        }
        #[cfg(feature = "plugin-opener")]
        {
            app = app.plugin(OpenerPlugin::new());
        }
        #[cfg(feature = "plugin-deep-link")]
        {
            app = app.plugin(DeepLinkPlugin::new());
        }
        #[cfg(feature = "plugin-shortcut")]
        {
            app = app.plugin(GlobalShortcutPlugin::new());
        }
        #[cfg(feature = "plugin-updater")]
        {
            app = app.plugin(UpdaterPlugin::new());
        }
        app
    }
}
