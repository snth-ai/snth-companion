package daemon

import _ "embed"

// assets.go — embedded menu-bar icon bitmaps.
//
// heart / heart.fill are the macOS SF Symbols rendered to template PNGs
// (pure black shape on transparent, @2x). Passed to
// systray.SetTemplateIcon so the OS tints them to match a light or dark
// menu bar automatically. Empty heart = disconnected, filled = connected.

//go:embed assets/heart.png
var iconHeartEmpty []byte

//go:embed assets/heart.fill.png
var iconHeartFill []byte

// appIconPNG is the 512×512 brand app icon (purple squircle + white
// lightning mark). Used to set the Dock icon at runtime for the
// bare-binary dev deploy, which has no .app bundle / Info.plist and
// would otherwise show a generic icon. The .app distribution build
// gets its icon from packaging/macos/AppIcon.icns instead.
//
//go:embed assets/appicon-512.png
var appIconPNG []byte
