#import <AppKit/AppKit.h>

// Called from GPUI's main thread. NSApplication retains its current icon.
bool telinha_set_application_icon(const unsigned char *png, size_t length) {
    @autoreleasepool {
        NSData *data = [NSData dataWithBytes:png length:length];
        NSImage *image = [[NSImage alloc] initWithData:data];
        if (!image || !NSApp) return false;
        [NSApp setApplicationIconImage:image];
        return true;
    }
}
