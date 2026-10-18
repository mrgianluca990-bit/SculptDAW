// Sculpt DAW: native Open Document Apple Event shim for Finder .scdaw files.
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h> // SCULPT_CORE22_FINDER_APPKIT

static void (*sculpt_open_callback)(const char *) = NULL;

@interface SculptOpenDocumentSink : NSObject
- (void)sculptInstallFinderHandler:(NSNotification *)event; // SCULPT_CORE22_FINDER_COLD_BOOT
- (void)handleOpenDocuments:(NSAppleEventDescriptor *)event
                 withReply:(NSAppleEventDescriptor *)reply;
@end

@implementation SculptOpenDocumentSink
// SCULPT_CORE22_FINDER_LAUNCH_OBSERVER: AppKit can reset odoc at startup.
- (void)sculptInstallFinderHandler:(NSNotification *)notification {
    (void)notification;
    [[NSAppleEventManager sharedAppleEventManager]
        setEventHandler:self andSelector:@selector(handleOpenDocuments:withReply:)
          forEventClass:0x61657674 andEventID:0x6f646f63];
}
- (void)handleOpenDocuments:(NSAppleEventDescriptor *)event
                 withReply:(NSAppleEventDescriptor *)reply {
    (void)reply;
    NSAppleEventDescriptor *items = [event paramDescriptorForKeyword:0x2d2d2d2d]; // '----'
    for (NSInteger i = 1; i <= [items numberOfItems]; ++i) {
        NSAppleEventDescriptor *item = [items descriptorAtIndex:i];
        NSURL *url = [item fileURLValue];
        if (!url) {
            NSString *raw = [item stringValue];
            if ([raw hasPrefix:@"file:"]) url = [NSURL URLWithString:raw];
            else if ([raw hasPrefix:@"/"]) url = [NSURL fileURLWithPath:raw];
        }
        if (url && url.isFileURL && sculpt_open_callback) {
            sculpt_open_callback(url.fileSystemRepresentation);
        }
    }
}
@end

void sculpt_install_document_handler(void (*callback)(const char *)) {
    // Strongly retain the target. NSAppleEventManager doesn't retain targets.
    static SculptOpenDocumentSink *sink = nil;
    if (!sink) {
        sink = [SculptOpenDocumentSink new];
        // SCULPT_CORE22_COLD_REGISTRATION: install before Finder's odoc event.
        [[NSNotificationCenter defaultCenter] addObserver:sink
            selector:@selector(sculptInstallFinderHandler:)
            name:NSApplicationWillFinishLaunchingNotification object:nil];
        [[NSNotificationCenter defaultCenter] addObserver:sink
            selector:@selector(sculptInstallFinderHandler:)
            name:NSApplicationDidFinishLaunchingNotification object:nil];
    }
    sculpt_open_callback = callback;
    [[NSAppleEventManager sharedAppleEventManager]
        setEventHandler:sink
           andSelector:@selector(handleOpenDocuments:withReply:)
         forEventClass:0x61657674  // 'aevt'
            andEventID:0x6f646f63]; // 'odoc'
}
