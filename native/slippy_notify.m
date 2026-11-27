#import <AppKit/AppKit.h>
#import <UserNotifications/UserNotifications.h>

@interface HexSlippyNotifications : NSObject <UNUserNotificationCenterDelegate>
@end

@implementation HexSlippyNotifications
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
       willPresentNotification:(UNNotification *)notification
         withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completion {
    completion(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionList);
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

static UNUserNotificationCenter *notification_center(void) {
    if (NSBundle.mainBundle.bundleIdentifier.length == 0) {
        return nil;
    }
    return UNUserNotificationCenter.currentNotificationCenter;
}

void hex_slippy_notifications_install(void) {
    UNUserNotificationCenter *center = notification_center();
    if (center) {
        static HexSlippyNotifications *delegate;
        delegate = [HexSlippyNotifications new];
        center.delegate = delegate;
    }
}

void hex_slippy_notifications_authorize(void) {
    static dispatch_once_t once;
    UNUserNotificationCenter *center = notification_center();
    if (center) {
        dispatch_once(&once, ^{
            [center requestAuthorizationWithOptions:UNAuthorizationOptionAlert
                                  completionHandler:^(__unused BOOL granted, __unused NSError *error) {}];
        });
    }
}

void hex_slippy_notify(const char *identifier, const char *title, const char *body, const char *link) {
    @autoreleasepool {
        UNUserNotificationCenter *center = notification_center();
        if (!center) {
            return;
        }
        UNMutableNotificationContent *content = [UNMutableNotificationContent new];
        content.title = [NSString stringWithUTF8String:title];
        content.body = [NSString stringWithUTF8String:body];
        content.userInfo = @{@"slippyURL": [NSString stringWithUTF8String:link]};
        [center addNotificationRequest:[UNNotificationRequest
            requestWithIdentifier:[NSString stringWithUTF8String:identifier]
                           content:content
                           trigger:nil]
                 withCompletionHandler:nil];
    }
}
