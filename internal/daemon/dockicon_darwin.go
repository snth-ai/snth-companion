//go:build darwin

package daemon

// dockicon_darwin.go — sets the macOS Dock icon at runtime.
//
// The dev deploy ships a bare binary (no .app bundle, no Info.plist),
// so macOS shows a generic executable icon in the Dock. Setting
// NSApplication.applicationIconImage at startup overrides that with the
// SNTH brand mark. The notarized .app distribution build gets its icon
// from packaging/macos/AppIcon.icns via CFBundleIconFile and does not
// need this — but calling it there too is harmless.

/*
#cgo CFLAGS: -x objective-c
#cgo LDFLAGS: -framework Cocoa
#import <Cocoa/Cocoa.h>

static void setDockIcon(const void *bytes, int len) {
    @autoreleasepool {
        NSData *data = [NSData dataWithBytes:bytes length:len];
        NSImage *img = [[NSImage alloc] initWithData:data];
        if (img != nil) {
            [NSApplication sharedApplication].applicationIconImage = img;
        }
    }
}
*/
import "C"

import "unsafe"

// SetDockIcon overrides the process's Dock icon with the given PNG
// bytes. No-op on empty input. Must be called on (or after) the main
// thread has an NSApplication — safe to call once at startup.
func SetDockIcon(png []byte) {
	if len(png) == 0 {
		return
	}
	C.setDockIcon(unsafe.Pointer(&png[0]), C.int(len(png)))
}

// SetBrandDockIcon applies the embedded SNTH brand icon.
func SetBrandDockIcon() { SetDockIcon(appIconPNG) }
