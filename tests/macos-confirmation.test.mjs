import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const gui = readFileSync(fileURLToPath(new URL("../install-wor-gui.sh", import.meta.url)), "utf8");
const confirmation = gui.match(/confirm_jxa="\$\(wor_jxa_window_lib; cat <<'JXA'\n([\s\S]*?)\nJXA/);
assert.ok(confirmation, "The confirmation dialog renderer is missing");
const source = confirmation[1];
const sizingStart = source.indexOf("const width = Math.min(");
const sizingEnd = source.indexOf("const content = window.contentView", sizingStart);
assert.ok(sizingStart >= 0 && sizingEnd > sizingStart);

function layout(rowCount, screenHeight, { screenWidth = 1470, titleBarHeight = 28, borderHeight = 4 } = {}) {
  let scroller = null;
  const context = vm.createContext({
    rows: Array.from({ length: rowCount }, () => ({ label: "Setting", value: "Value" })),
    rowHeight: 24, widestValue: 400, windowTitle: "Review fixture", controller: {},
    screenFrame: { size: { width: screenWidth, height: screenHeight } },
    worMakeWindow(options) {
      return {
        frame: { size: { width: options.width, height: options.height + titleBarHeight } },
        contentView: { frame: { size: { width: options.width, height: options.height } }, addSubview() {} },
        setContentSize(size) {
          this.contentView.frame.size = size;
          this.frame.size = { width: size.width, height: size.height + titleBarHeight };
        },
      };
    },
    $: {
      NSMakeRect: (x, y, width, height) => ({ origin: { x, y }, size: { width, height } }),
      NSMakeSize: (width, height) => ({ width, height }),
      NSMakePoint: (x, y) => ({ x, y }),
      NSBezelBorder: 2, NSScrollerStyleLegacy: 0, NSTextAlignmentRight: 2, NSLineBreakByTruncatingMiddle: 5,
      NSFont: { systemFontOfSizeWeight: () => ({}) },
      NSColor: { secondaryLabelColor: "secondary" },
      NSTextField: { labelWithString: (text) => ({ text }) },
      NSView: { alloc: { initWithFrame(frame) {
        return {
          frame, subviews: [],
          addSubview(view) { this.subviews.push(view); },
          scrollPoint(point) { this.scrollPosition = point; },
        };
      } } },
      NSScrollView: { alloc: { initWithFrame(frame) {
        scroller = {
          frame, contentView: {},
          reflectScrolledClipView() {},
          get contentSize() {
            const border = this.borderType === 2 ? borderHeight : 0;
            return {
              width: this.frame.size.width - border - (this.hasVerticalScroller ? 17 : 0),
              height: this.frame.size.height - border - (this.hasHorizontalScroller ? 17 : 0),
            };
          },
        };
        return scroller;
      } } },
    },
  });
  vm.runInContext(source.slice(sizingStart, sizingEnd), context);
  const documentStart = source.indexOf("const documentWidth =");
  const documentEnd = source.indexOf("const guidance =", documentStart);
  if (documentStart >= 0 && documentEnd > documentStart) {
    vm.runInContext(`const content = window.contentView;\n${source.slice(documentStart, documentEnd)}`, context);
  }
  return {
    window: vm.runInContext("window", context),
    scroller,
    settingsHeight: vm.runInContext("typeof settingsHeight === 'undefined' ? null : settingsHeight", context),
    document: vm.runInContext("typeof documentView === 'undefined' ? null : documentView", context),
  };
}

describe("Review flash settings content fit", () => {
  for (const [rowCount, expectedWindowHeight] of [[16, 652], [18, 700], [20, 748]]) {
    it(`shows all ${rowCount} rows without scrolling on a roomy screen`, () => {
      const result = layout(rowCount, 868);
      assert.equal(result.window.frame.size.height, expectedWindowHeight);
      assert.ok(result.scroller);
      assert.equal(result.scroller.hasVerticalScroller, false);
      assert.equal(result.scroller.hasHorizontalScroller, false);
      assert.equal(result.scroller.contentSize.height, rowCount * 24 + 16);
      assert.equal(result.settingsHeight, rowCount * 24 + 16);
      assert.equal(result.document.frame.size.height, result.scroller.contentSize.height);
      assert.equal(result.document.frame.size.width, result.scroller.contentSize.width);
      assert.equal(result.document.subviews.length, rowCount * 2);
      for (const view of result.document.subviews) {
        assert.ok(view.frame.origin.y >= 0);
        assert.ok(view.frame.origin.y + view.frame.size.height <= result.document.frame.size.height);
      }
    });
  }

  it("does not scroll when the panel fits exactly including border and window chrome", () => {
    const result = layout(18, 700);
    assert.equal(result.window.frame.size.height, 700);
    assert.equal(result.scroller.hasVerticalScroller, false);
    assert.equal(result.scroller.contentSize.height, result.settingsHeight);
  });

  for (const screenHeight of [699, 600]) {
    it(`shows a persistent vertical scrollbar only for real overflow at ${screenHeight} points`, () => {
      const result = layout(18, screenHeight);
      assert.equal(result.window.frame.size.height, screenHeight);
      assert.equal(result.scroller.hasVerticalScroller, true);
      assert.equal(result.scroller.hasHorizontalScroller, false);
      assert.equal(result.scroller.autohidesScrollers, false);
      assert.equal(result.scroller.scrollerStyle, 0);
      assert.equal(result.settingsHeight - result.scroller.contentSize.height, 700 - screenHeight);
      assert.equal(result.scroller.frame.origin.y, 86, "Result controls must stay below the panel");
      assert.equal(result.document.scrollPosition.y, 700 - screenHeight);
    });
  }

  it("accounts for actual title-bar and panel-border sizes", () => {
    const result = layout(18, 700, { titleBarHeight: 32, borderHeight: 6 });
    assert.equal(result.window.frame.size.height, 700);
    assert.equal(result.scroller.hasVerticalScroller, true);
    assert.equal(result.settingsHeight - result.scroller.contentSize.height, 6);
  });

  it("does not make short settings lists artificially scroll", () => {
    const result = layout(2, 868);
    assert.equal(result.window.contentView.frame.size.height, 320);
    assert.equal(result.scroller.hasVerticalScroller, false);
    assert.ok(result.scroller.contentSize.height > result.settingsHeight);
    assert.equal(result.document.frame.size.height, result.scroller.contentSize.height);
    assert.equal(result.document.scrollPosition.y, 0);
  });

  it("uses the actual viewport width and starts overflow at the first row", () => {
    assert.match(source, /const documentWidth = Number\(scrollView\.contentSize\.width\)/);
    assert.match(source, /const documentHeight = Math\.max\(settingsHeight, Number\(scrollView\.contentSize\.height\)\)/);
    assert.match(source, /documentView\.scrollPoint\(\$\.NSMakePoint\(0, Math\.max\(0, documentHeight - Number\(scrollView\.contentSize\.height\)\)\)\)/);
    assert.match(source, /scrollView\.reflectScrolledClipView\(scrollView\.contentView\)/);
  });

  it("keeps the erase warning and confirmation actions outside the scrolling panel", () => {
    for (const control of ["heading", "target", "warning", "guidance", "flashButton", "backButton", "advancedButton"]) {
      assert.ok(source.includes(`content.addSubview(${control})`), `${control} must stay outside the settings document`);
    }
    assert.match(source, /All data on the target drive will be erased\./);
    assert.match(source, /selectedValue = 'Flash'/);
    assert.match(source, /selectedValue = 'Advanced'/);
    assert.match(source, /selectedValue = 'Back'/);
  });
});
