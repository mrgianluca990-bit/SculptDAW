#import <Cocoa/Cocoa.h>
#import <Carbon/Carbon.h>
#import <unistd.h>

@interface SculptDocumentEventHandler : NSObject
+ (instancetype)shared;
- (void)handleOpenDocuments:(NSAppleEventDescriptor *)event withReplyEvent:(NSAppleEventDescriptor *)reply;
@end

@implementation SculptDocumentEventHandler
+ (instancetype)shared {
    static SculptDocumentEventHandler *handler;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ handler=[SculptDocumentEventHandler new]; });
    return handler;
}
- (void)handleOpenDocuments:(NSAppleEventDescriptor *)event withReplyEvent:(NSAppleEventDescriptor *)reply {
    (void)reply;
    NSAppleEventDescriptor *list=[event paramDescriptorForKeyword:keyDirectObject];
    if (!list) return;
    NSString *spool=[NSTemporaryDirectory() stringByAppendingPathComponent:
                     [NSString stringWithFormat:@"sculpt-open-documents-%d.txt",getpid()]];
    for (NSInteger i=1;i<=[list numberOfItems];i++) {
        NSAppleEventDescriptor *desc=[list descriptorAtIndex:i];
        NSAppleEventDescriptor *urlDesc=[desc coerceToDescriptorType:typeFileURL];
        NSString *value=[urlDesc stringValue];
        NSString *filePath=nil;
        if (value.length) {
            NSURL *url=[NSURL URLWithString:value];
            if (url.isFileURL) filePath=url.path;
        }
        if (!filePath.length) {
            NSString *fallback=[desc stringValue];
            if ([fallback hasPrefix:@"/"]) filePath=fallback;
        }
        if (!filePath.length) continue;
        // Each line is a path. Real Finder-open filenames with newline are
        // exceptional; avoid a risky custom object protocol on the audio thread.
        NSMutableData *bytes=[[filePath dataUsingEncoding:NSUTF8StringEncoding] mutableCopy];
        const char terminator='\n';
        [bytes appendBytes:&terminator length:1];
        if (![[NSFileManager defaultManager] fileExistsAtPath:spool]) {
            [[NSFileManager defaultManager] createFileAtPath:spool contents:nil attributes:nil];
        }
        NSFileHandle *handle=[NSFileHandle fileHandleForWritingAtPath:spool];
        if (!handle) continue;
        @try {
            [handle seekToEndOfFile];
            [handle writeData:bytes];
        } @finally { [handle closeFile]; }
    }
}
@end

__attribute__((constructor)) static void sculpt_register_finder_document_events(void) {
    @autoreleasepool {
        [[NSAppleEventManager sharedAppleEventManager]
         setEventHandler:[SculptDocumentEventHandler shared]
         andSelector:@selector(handleOpenDocuments:withReplyEvent:)
         forEventClass:kCoreEventClass andEventID:kAEOpenDocuments];
    }
}
