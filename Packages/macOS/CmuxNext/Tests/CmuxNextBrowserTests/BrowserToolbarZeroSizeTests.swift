import AppKit
@testable import CmuxNextBrowser
import CmuxNextDesign
import CmuxNextTestSupport
import Foundation
import Testing

/// A browser pane's chrome first lays out in a content area of zero size (`PaneContentAreaView()` starts at
/// `.zero`; its pane gives it a frame later). The toolbar row cannot fit in zero points, and with required links
/// end to end AppKit broke some of them (the chip-to-field gap, the badges' edge, the star's width; the user's
/// nightly log, 2026-11-11 19:56) and the chrome's autoresized frame kept the overflow. Once the pane had its
/// width, a long URL ran past the omnibar's star and under the extension buttons to the window's edge (user,
/// 2026-11-11 20:05: "omnibar overflow text has issue"). The row and the omnibar inside it must come back whole.
@MainActor @Suite(.serialized, .closesItsWindows) struct BrowserToolbarZeroSizeTests {
    static let longURL = URL(string: "https://tandemspace.com/office/9428bd16-cf52-4f24-99e8-dfd2abca5f9e"
        + "?from=admin_matchmaking&utm_source=a-long-query-string-that-overflows-the-omnibar&utm_campaign=more")!

    /// With `inWindow`, the content area is in a window when the chrome joins it; without, the area is not in a
    /// window yet (a pane built before it is shown).
    @Test(arguments: [true, false])
    func theOmnibarComesBackWholeAfterAZeroSizeFirstLayout(inWindow: Bool) async throws {
        let tab = MockBrowserEngine(kind: .cef).makeMockTab(BrowserTabConfiguration(initialURL: Self.longURL))
        tab.load(Self.longURL)
        let pane = BrowserPaneChrome()
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 700), styleMask: [.titled],
                              backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer {
            window.close()
            pane.drop()
            tab.close()
        }
        let root = try #require(window.contentView)
        let area = PaneContentAreaView()
        if inWindow { root.addSubview(area) }
        let chrome = pane.bind(tab, state: BrowserChromeTabState())
        chrome.frame = area.bounds
        chrome.autoresizingMask = [.width, .height]
        area.addSubview(chrome)
        area.layoutSubtreeIfNeeded()
        if !inWindow { root.addSubview(area) }

        area.frame = NSRect(x: 0, y: 0, width: 1100, height: 600)
        root.layoutSubtreeIfNeeded()

        let bar = chrome.addressBar
        let field = bar.field
        #expect(chrome.frame == area.bounds, "the chrome fills its content area: \(chrome.frame) in \(area.bounds)")
        let buttons = chrome.toolbarButtons.convert(chrome.toolbarButtons.bounds, to: chrome)
        #expect(buttons.maxX <= chrome.bounds.maxX, "the toolbar buttons stay in the pane: \(buttons) in \(chrome.bounds)")
        let barFrame = bar.convert(bar.bounds, to: chrome)
        #expect(barFrame.maxX <= buttons.minX, "the omnibar ends before the toolbar buttons: \(barFrame), buttons \(buttons)")
        #expect(field.frame.minX >= bar.chip.frame.maxX, "the address starts after the page-info chip: \(field.frame), chip \(bar.chip.frame)")
        #expect(field.frame.maxX <= bar.bounds.maxX - OmnibarStyle.trailingPadding + 0.5,
                "the address ends inside the omnibar: field \(field.frame), bar \(bar.bounds)")
        if !bar.starButton.isHidden {
            #expect(field.frame.maxX <= bar.starButton.convert(bar.starButton.bounds, to: bar).minX,
                    "the address ends before the star: field \(field.frame)")
        }
    }
}
