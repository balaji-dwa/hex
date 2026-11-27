#import <AppKit/AppKit.h>
#import <UserNotifications/UserNotifications.h>

// Implemented in Rust (`src/slippy/notify.rs`): writes to Hex's process.log.
// `level` 0 is info, anything else a warning.
extern void hex_slippy_notify_log(int level, const char *message);

static const UNAuthorizationOptions kSlippyOptions =
    UNAuthorizationOptionAlert | UNAuthorizationOptionSound;

static void slippy_log(int level, NSString *format, ...) NS_FORMAT_FUNCTION(2, 3);
static void slippy_log(int level, NSString *format, ...) {
    va_list args;
    va_start(args, format);
    NSString *message = [[NSString alloc] initWithFormat:format arguments:args];
    va_end(args);
    hex_slippy_notify_log(level, message.UTF8String ?: "");
}

@interface HexSlippyNotifications : NSObject <UNUserNotificationCenterDelegate>
@end

@implementation HexSlippyNotifications
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
       willPresentNotification:(UNNotification *)notification
         withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completion {
    completion(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionList |
               UNNotificationPresentationOptionSound);
}

- (void)userNotificationCenter:(UNUserNotificationCenter *)center
 didReceiveNotificationResponse:(UNNotificationResponse *)response
          withCompletionHandler:(void (^)(void))completion {
    if ([response.actionIdentifier isEqualToString:UNNotificationDefaultActionIdentifier]) {
        NSString *link = response.notification.request.content.userInfo[@"slippyURL"];
        NSURL *url = link ? [NSURL URLWithString:link] : nil;
        if (url) {
            dispatch_async(dispatch_get_main_queue(), ^{
                [NSWorkspace.sharedWorkspace openURL:url];
            });
        }
    }
    completion();
}
@end

static BOOL bundled(void) {
    return NSBundle.mainBundle.bundleIdentifier.length > 0;
}

/// Runs `block` on the main queue: inline when already there (the install,
/// before NSApplication runs its loop), queued from the `hex-slippy` thread.
static void on_main(dispatch_block_t block) {
    if (NSThread.isMainThread) {
        block();
    } else {
        dispatch_async(dispatch_get_main_queue(), block);
    }
}

/// Attaches the one delegate. Called at install, authorize, and every post,
/// so it holds even if something reset it once NSApplication finished
/// launching. Main thread only.
static void ensure_delegate(UNUserNotificationCenter *center) {
    static HexSlippyNotifications *delegate;
    if (!delegate) {
        delegate = [HexSlippyNotifications new];
    }
    if (center.delegate != delegate) {
        center.delegate = delegate;
    }
}

static UNUserNotificationCenter *notification_center(void) {
    if (!bundled()) {
        return nil;
    }
    UNUserNotificationCenter *center = UNUserNotificationCenter.currentNotificationCenter;
    ensure_delegate(center);
    return center;
}

static void request_authorization(UNUserNotificationCenter *center,
                                  void (^then)(BOOL granted)) {
    [center requestAuthorizationWithOptions:kSlippyOptions
                          completionHandler:^(BOOL granted, NSError *error) {
        if (error) {
            slippy_log(1, @"Slippy notification authorization failed: %@",
                       error.localizedDescription);
        } else {
            slippy_log(granted ? 0 : 1, @"Slippy notification authorization granted=%d",
                       granted);
        }
        if (then) {
            then(granted);
        }
    }];
}

static void add_request(UNUserNotificationCenter *center, NSString *identifier, NSString *title,
                        NSString *body, NSString *link) {
    UNMutableNotificationContent *content = [UNMutableNotificationContent new];
    content.title = title;
    content.body = body;
    content.sound = UNNotificationSound.defaultSound;
    content.userInfo = @{@"slippyURL": link};
    UNNotificationRequest *request = [UNNotificationRequest requestWithIdentifier:identifier
                                                                          content:content
                                                                          trigger:nil];
    [center addNotificationRequest:request
             withCompletionHandler:^(NSError *error) {
        if (error) {
            slippy_log(1, @"Slippy notification %@ failed: %@", identifier,
                       error.localizedDescription);
        } else {
            slippy_log(0, @"Slippy notification %@ accepted by Notification Center", identifier);
        }
    }];
}

void hex_slippy_notifications_install(void) {
    if (!bundled()) {
        return;
    }
    on_main(^{
        (void)notification_center();
    });
}

void hex_slippy_notifications_authorize(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        if (!bundled()) {
            slippy_log(1, @"Slippy notifications off: Hex has no bundle identifier");
            return;
        }
        on_main(^{
            request_authorization(notification_center(), nil);
        });
    });
}

void hex_slippy_notify(const char *identifier, const char *title, const char *body,
                       const char *link) {
    @autoreleasepool {
        if (!bundled()) {
            slippy_log(1, @"Slippy notification skipped: Hex has no bundle identifier");
            return;
        }
        // Copy now: the caller's strings live only for this call.
        NSString *ident = [NSString stringWithUTF8String:identifier] ?: @"hex.slippy";
        NSString *heading = [NSString stringWithUTF8String:title] ?: @"Slippy";
        NSString *text = [NSString stringWithUTF8String:body] ?: @"";
        NSString *url = [NSString stringWithUTF8String:link] ?: @"";
        on_main(^{
            UNUserNotificationCenter *center = notification_center();
            [center getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings *settings) {
                slippy_log(0,
                           @"Slippy notification %@: authorizationStatus=%ld alertSetting=%ld "
                           @"alertStyle=%ld soundSetting=%ld",
                           ident, (long)settings.authorizationStatus,
                           (long)settings.alertSetting, (long)settings.alertStyle,
                           (long)settings.soundSetting);
                UNAuthorizationStatus status = settings.authorizationStatus;
                if (status == UNAuthorizationStatusNotDetermined) {
                    request_authorization(center, ^(BOOL granted) {
                        if (granted) {
                            add_request(center, ident, heading, text, url);
                        }
                    });
                } else if (status == UNAuthorizationStatusDenied) {
                    slippy_log(1,
                               @"Slippy notification %@ not shown: notifications for Hex are "
                               @"off in System Settings",
                               ident);
                } else {
                    add_request(center, ident, heading, text, url);
                }
            }];
        });
    }
}
