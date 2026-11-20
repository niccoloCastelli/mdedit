//! Files opened from Finder on macOS.
//!
//! LaunchServices does not pass documents as command-line arguments: it
//! sends an `odoc` ("open documents") Apple Event, both when it launches the
//! app and when the app is already running. winit does not surface that
//! event, so we install our own handler.
//!
//! AppKit installs its default Apple Event handlers just before posting
//! `NSApplicationWillFinishLaunchingNotification`, and Apple's guidance is to
//! replace them from there, which is still before the launch-time `odoc`
//! arrives. [`install`] therefore only observes that notification; the
//! observer then registers the `odoc` handler. Paths go into a channel that
//! [`App::new`](crate::app::App) drains for the files the app was launched
//! with and [`subscription`] forwards afterwards.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use iced::Subscription;
use iced::futures::SinkExt;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSString};
use tokio::sync::mpsc;

/// `kCoreEventClass`, `kAEOpenDocuments` and `keyDirectObject`.
const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

type Batch = Vec<PathBuf>;

static SENDER: OnceLock<mpsc::UnboundedSender<Batch>> = OnceLock::new();
/// Taken by [`subscription`] the first time it runs.
static RECEIVER: Mutex<Option<mpsc::UnboundedReceiver<Batch>>> = Mutex::new(None);

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MdeditOpenDocumentsHandler"]
    struct Handler;

    impl Handler {
        #[unsafe(method(applicationWillFinishLaunching:))]
        fn will_finish_launching(&self, _note: &NSNotification) {
            unsafe {
                NSAppleEventManager::sharedAppleEventManager().setEventHandler_andSelector_forEventClass_andEventID(
                    self,
                    sel!(handleOpenDocuments:withReplyEvent:),
                    CORE_EVENT_CLASS,
                    OPEN_DOCUMENTS,
                );
            }
        }

        #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
        fn handle_open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            let Some(list) = event.paramDescriptorForKeyword(DIRECT_OBJECT) else { return };
            // Apple Event lists are 1-based.
            let paths: Batch = (1..=list.numberOfItems())
                .filter_map(|i| list.descriptorAtIndex(i))
                .filter_map(|d| d.fileURLValue())
                .filter_map(|url| url.path())
                .map(|p| PathBuf::from(p.to_string()))
                .collect();
            if !paths.is_empty()
                && let Some(tx) = SENDER.get()
            {
                let _ = tx.send(paths);
            }
        }
    }
);

/// Start listening for `odoc` events. Call once from `main`, before the
/// event loop starts.
pub fn install() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let (tx, rx) = mpsc::unbounded_channel();
    if SENDER.set(tx).is_err() {
        return;
    }
    *RECEIVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(rx);

    let handler: Retained<Handler> = unsafe { msg_send![Handler::alloc(mtm), init] };
    let name = NSString::from_str("NSApplicationWillFinishLaunchingNotification");
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &handler,
            sel!(applicationWillFinishLaunching:),
            Some(&name),
            None,
        );
    }
    // The notification centre and the Apple Event manager do not retain
    // their targets; the handler lives as long as the app.
    std::mem::forget(handler);
}

/// Files that arrived before the app state existed (the launch `odoc`).
pub fn take_pending() -> Batch {
    let mut guard = RECEIVER.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    if let Some(rx) = guard.as_mut() {
        while let Ok(batch) = rx.try_recv() {
            out.extend(batch);
        }
    }
    out
}

/// Files opened from Finder while the app is running.
pub fn subscription() -> Subscription<Batch> {
    Subscription::run(|| {
        iced::stream::channel(4, async |mut out| {
            let rx = RECEIVER.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(mut rx) = rx {
                while let Some(batch) = rx.recv().await {
                    if out.send(batch).await.is_err() {
                        break;
                    }
                }
            }
            std::future::pending::<()>().await;
        })
    })
}
