//! Built-in native desktop capability plugins implementing [`crate::Plugin`].

#[cfg(feature = "plugin-clipboard")]
pub mod clipboard;
#[cfg(feature = "plugin-dialog")]
pub mod dialog;
#[cfg(feature = "plugin-fs")]
pub mod fs;
#[cfg(feature = "plugin-notification")]
pub mod notification;

#[cfg(feature = "plugin-clipboard")]
pub use clipboard::ClipboardPlugin;
#[cfg(feature = "plugin-dialog")]
pub use dialog::DialogPlugin;
#[cfg(feature = "plugin-fs")]
pub use fs::AppFsPlugin;
#[cfg(feature = "plugin-notification")]
pub use notification::NotificationPlugin;

/// Convenience bundle registering all enabled built-in FORGE desktop plugins
/// (`DialogPlugin`, `ClipboardPlugin`, `NotificationPlugin`, and `AppFsPlugin`).
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
        app
    }
}
