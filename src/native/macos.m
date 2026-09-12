#import <AppKit/AppKit.h>
#include "bridge.h"
#include <stdlib.h>
#include <string.h>

static NSWindow *window;
static NSScrollView *scroll;
static NSTextView *editor;
static NSTextField *statusLabel;
static NSSlider *filePosition;
static BOOL updating, busy, readonlyDocument, smokeTest;
static NSString *currentFont;
static double currentSize;
static BOOL currentSpell;
static int currentLanguage;

static NSString *S(const char *text) { return [NSString stringWithUTF8String:text] ?: @""; }
static NSString *L(int id) { return S(rp_label(id)); }

@interface RavnTextView : NSTextView
@end
@implementation RavnTextView
- (void)toggleContinuousSpellChecking:(id)sender {
    [super toggleContinuousSpellChecking:sender]; rp_action(RP_SPELL);
}
- (void)changeFont:(id)sender {
    [super changeFont:sender];
    rp_font(self.font.fontName.UTF8String, self.font.pointSize);
}
- (NSDragOperation)draggingEntered:(id<NSDraggingInfo>)sender {
    if ([sender.draggingPasteboard canReadObjectForClasses:@[[NSURL class]] options:@{NSPasteboardURLReadingFileURLsOnlyKey:@YES}]) return busy ? NSDragOperationNone : NSDragOperationCopy;
    return [super draggingEntered:sender];
}
- (BOOL)performDragOperation:(id<NSDraggingInfo>)sender {
    NSArray<NSURL *> *urls = [sender.draggingPasteboard readObjectsForClasses:@[[NSURL class]] options:@{NSPasteboardURLReadingFileURLsOnlyKey:@YES}];
    if (urls.count) { for (NSURL *url in urls) rp_open(url.path.UTF8String); return YES; }
    return [super performDragOperation:sender];
}
@end

@interface RavnDelegate : NSObject <NSApplicationDelegate, NSWindowDelegate, NSTextViewDelegate, NSMenuItemValidation>
@property NSPanel *settings;
@property NSPopUpButton *languages;
@property NSButton *spelling;
- (void)command:(NSMenuItem *)sender;
- (void)showSettings:(id)sender;
@end
static RavnDelegate *delegate;

static NSMenuItem *item(NSMenu *menu, NSString *title, SEL action, NSString *key, id target, NSInteger tag) {
    NSMenuItem *entry = [[NSMenuItem alloc] initWithTitle:title action:action keyEquivalent:key];
    entry.target = target; entry.tag = tag; [menu addItem:entry]; return entry;
}
static void command(NSMenu *menu, int id, NSString *key) { item(menu, L(id), @selector(command:), key, delegate, id); }
static NSMenu *submenu(NSMenu *bar, NSString *title) {
    NSMenu *menu = [[NSMenu alloc] initWithTitle:title];
    NSMenuItem *entry = [[NSMenuItem alloc] initWithTitle:title action:nil keyEquivalent:@""];
    entry.submenu = menu; [bar addItem:entry]; return menu;
}
void rp_rebuild_menus(void) {
    NSMenu *bar = [NSMenu new];
    NSMenu *app = submenu(bar, @"RavnPad");
    command(app, RP_ABOUT, @"");
    [app addItem:NSMenuItem.separatorItem];
    item(app, [L(RP_SETTINGS) stringByAppendingString:@"…"], @selector(showSettings:), @",", delegate, 0);
    NSMenu *services = submenu(app, L(RP_SERVICES)); NSApp.servicesMenu = services;
    [app addItem:NSMenuItem.separatorItem];
    item(app, L(RP_HIDE), @selector(hide:), @"h", nil, 0);
    NSMenuItem *hide = item(app, L(RP_HIDE_OTHERS), @selector(hideOtherApplications:), @"h", nil, 0);
    hide.keyEquivalentModifierMask = NSEventModifierFlagCommand | NSEventModifierFlagOption;
    item(app, L(RP_SHOW_ALL), @selector(unhideAllApplications:), @"", nil, 0);
    [app addItem:NSMenuItem.separatorItem]; command(app, RP_QUIT, @"q");
    NSMenu *file = submenu(bar, L(RP_FILE));
    command(file, RP_NEW, @"n"); command(file, RP_OPEN, @"o");
    NSMenu *recent = submenu(file, L(RP_RECENT));
    for (NSURL *url in NSDocumentController.sharedDocumentController.recentDocumentURLs) {
        NSMenuItem *entry = item(recent, url.lastPathComponent, @selector(openRecent:), @"", delegate, 0);
        entry.representedObject = url;
    }
    [file addItem:NSMenuItem.separatorItem]; command(file, RP_SAVE, @"s");
    command(file, RP_SAVE_AS, @"S"); command(file, RP_RECOVERY, @"");
    item(file, L(RP_CLOSE), @selector(performClose:), @"w", nil, 0);
    NSMenu *edit = submenu(bar, L(RP_EDIT));
    item(edit,L(RP_UNDO),@selector(undo:),@"z",nil,0);
    item(edit,L(RP_REDO),@selector(redo:),@"Z",nil,0);
    [edit addItem:NSMenuItem.separatorItem];
    item(edit,L(RP_CUT),@selector(cut:),@"x",nil,0);
    item(edit,L(RP_COPY),@selector(copy:),@"c",nil,0);
    item(edit,L(RP_PASTE),@selector(paste:),@"v",nil,0);
    item(edit,L(RP_SELECT_ALL),@selector(selectAll:),@"a",nil,0);
    [edit addItem:NSMenuItem.separatorItem];
    item(edit,L(RP_FIND),@selector(performTextFinderAction:),@"f",nil,NSTextFinderActionShowFindInterface);
    item(edit,L(RP_REPLACE),@selector(performTextFinderAction:),@"",nil,NSTextFinderActionShowReplaceInterface);
    item(edit,L(RP_NEXT),@selector(performTextFinderAction:),@"g",nil,NSTextFinderActionNextMatch);
    item(edit,L(RP_PREVIOUS),@selector(performTextFinderAction:),@"G",nil,NSTextFinderActionPreviousMatch);
    NSMenu *view = submenu(bar, L(RP_WINDOW)); NSApp.windowsMenu = view;
    item(view,L(RP_MINIMIZE),@selector(performMiniaturize:),@"m",nil,0);
    item(view,L(RP_ZOOM),@selector(performZoom:),@"",nil,0);
    NSMenu *help = submenu(bar,L(RP_HELP)); command(help,RP_UPDATE,@""); NSApp.helpMenu=help;
    NSApp.mainMenu=bar;
}

@implementation RavnDelegate
- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    (void)notification;
    window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,900,600) styleMask:NSWindowStyleMaskTitled|NSWindowStyleMaskClosable|NSWindowStyleMaskMiniaturizable|NSWindowStyleMaskResizable backing:NSBackingStoreBuffered defer:NO];
    window.delegate=self; window.title=@"RavnPad"; window.minSize=NSMakeSize(420,300);
    window.releasedWhenClosed=NO; [window setFrameAutosaveName:@"RavnPadDocument"];
    NSView *content=window.contentView;
    scroll=[[NSScrollView alloc] initWithFrame:NSMakeRect(0,28,900,572)];
    scroll.autoresizingMask=NSViewWidthSizable|NSViewHeightSizable;
    scroll.hasVerticalScroller=YES; scroll.hasHorizontalScroller=NO; scroll.borderType=NSNoBorder;
    editor=[[RavnTextView alloc] initWithFrame:scroll.contentView.bounds];
    editor.autoresizingMask=NSViewWidthSizable;
    editor.minSize=NSMakeSize(0,scroll.contentSize.height); editor.maxSize=NSMakeSize(CGFLOAT_MAX,CGFLOAT_MAX);
    editor.verticallyResizable=YES; editor.horizontallyResizable=NO;
    editor.textContainer.containerSize=NSMakeSize(scroll.contentSize.width,CGFLOAT_MAX);
    editor.textContainer.widthTracksTextView=YES;
    editor.textContainerInset=NSMakeSize(12,12);
    editor.richText=NO; editor.importsGraphics=NO; editor.allowsUndo=YES;
    editor.usesFindBar=YES; editor.incrementalSearchingEnabled=YES;
    editor.automaticQuoteSubstitutionEnabled=NO; editor.automaticDashSubstitutionEnabled=NO;
    editor.automaticTextReplacementEnabled=NO; editor.automaticSpellingCorrectionEnabled=NO;
    editor.usesAdaptiveColorMappingForDarkAppearance=YES;
    editor.backgroundColor=NSColor.textBackgroundColor; editor.textColor=NSColor.textColor;
    editor.font=[NSFont monospacedSystemFontOfSize:15 weight:NSFontWeightRegular];
    editor.delegate=self; scroll.documentView=editor; [content addSubview:scroll];
    statusLabel=[NSTextField labelWithString:@""];
    statusLabel.frame=NSMakeRect(12,5,870,18); statusLabel.autoresizingMask=NSViewWidthSizable;
    statusLabel.font=[NSFont systemFontOfSize:NSFont.smallSystemFontSize]; statusLabel.textColor=NSColor.secondaryLabelColor;
    [content addSubview:statusLabel];
    filePosition=[[NSSlider alloc] initWithFrame:NSMakeRect(650,3,230,22)];
    filePosition.minValue=0; filePosition.maxValue=1; filePosition.target=self; filePosition.action=@selector(moveFile:);
    filePosition.continuous=NO; filePosition.autoresizingMask=NSViewMinXMargin; filePosition.hidden=YES;
    filePosition.accessibilityLabel=@"File position"; [content addSubview:filePosition];
    rp_rebuild_menus(); if(smokeTest) return; [window center]; [window makeKeyAndOrderFront:nil]; [window makeFirstResponder:editor];
    [NSApp activateIgnoringOtherApps:YES];
    [NSTimer scheduledTimerWithTimeInterval:0.15 target:self selector:@selector(tick:) userInfo:nil repeats:YES];
    rp_tick();
}
- (void)tick:(NSTimer *)timer { (void)timer; rp_tick(); }
- (void)textDidChange:(NSNotification *)notification { (void)notification; if (!updating) { window.documentEdited=YES; rp_changed(); } }
- (void)command:(NSMenuItem *)sender { rp_action((int)sender.tag); rp_tick(); }
- (void)openRecent:(NSMenuItem *)sender { rp_open([(NSURL *)sender.representedObject path].UTF8String); rp_tick(); }
- (void)moveFile:(NSSlider *)sender { rp_view(sender.doubleValue); }
- (BOOL)windowShouldClose:(NSWindow *)sender { (void)sender; rp_action(RP_QUIT); return NO; }
- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)sender { (void)sender; rp_action(RP_QUIT); return NSTerminateCancel; }
- (void)application:(NSApplication *)sender openFiles:(NSArray<NSString *> *)files {
    for (NSString *path in files) rp_open(path.UTF8String);
    [sender replyToOpenOrPrint:NSApplicationDelegateReplySuccess];
}
- (BOOL)validateMenuItem:(NSMenuItem *)item {
    if (item.action==@selector(command:)) return !busy && (!(item.tag==RP_SAVE || item.tag==RP_SAVE_AS) || !readonlyDocument);
    return YES;
}
- (void)changeLanguage:(NSPopUpButton *)sender { rp_action(100+(int)sender.indexOfSelectedItem); rp_tick(); }
- (void)changeSpelling:(NSButton *)sender { (void)sender; rp_action(RP_SPELL); rp_tick(); }
- (void)showFont:(id)sender { (void)sender; [window makeFirstResponder:editor]; [[NSFontManager sharedFontManager] setSelectedFont:editor.font isMultiple:NO]; [[NSFontManager sharedFontManager] orderFrontFontPanel:self]; }
- (void)showSettings:(id)sender {
    (void)sender;
    self.settings=[[NSPanel alloc] initWithContentRect:NSMakeRect(0,0,420,170) styleMask:NSWindowStyleMaskTitled|NSWindowStyleMaskClosable backing:NSBackingStoreBuffered defer:NO];
    self.settings.title=L(RP_SETTINGS); self.settings.releasedWhenClosed=NO;
    NSTextField *label=[NSTextField labelWithString:L(RP_LANGUAGE)]; label.frame=NSMakeRect(20,118,140,24); [self.settings.contentView addSubview:label];
    self.languages=[[NSPopUpButton alloc] initWithFrame:NSMakeRect(165,118,230,26) pullsDown:NO];
    for(int i=0;i<15;i++) [self.languages addItemWithTitle:L(100+i)];
    [self.languages selectItemAtIndex:currentLanguage];
    self.languages.target=self; self.languages.action=@selector(changeLanguage:); [self.settings.contentView addSubview:self.languages];
    NSButton *font=[NSButton buttonWithTitle:L(RP_FONT) target:self action:@selector(showFont:)]; font.frame=NSMakeRect(20,65,170,32); [self.settings.contentView addSubview:font];
    self.spelling=[NSButton checkboxWithTitle:L(RP_SPELL) target:self action:@selector(changeSpelling:)]; self.spelling.frame=NSMakeRect(20,22,370,26); self.spelling.state=currentSpell; [self.settings.contentView addSubview:self.spelling];
    [self.settings center]; [self.settings makeKeyAndOrderFront:nil];
}
@end

void rp_run(void) { @autoreleasepool { [NSApplication sharedApplication]; [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular]; delegate=[RavnDelegate new]; NSApp.delegate=delegate; [NSApp run]; } }
void rp_document(const char *text,size_t length,int readonly) {
    updating=YES;
    NSString *value=[[NSString alloc] initWithBytes:text length:length encoding:NSUTF8StringEncoding];
    editor.string=value ?: @""; [editor.undoManager removeAllActions];
    editor.editable=!readonly; [editor setSelectedRange:NSMakeRange(0,0)]; [editor scrollRangeToVisible:NSMakeRange(0,0)];
    updating=NO;
}
char *rp_copy_text(size_t *length) {
    NSData *bytes=[editor.string dataUsingEncoding:NSUTF8StringEncoding]; *length=bytes.length;
    char *out=malloc(*length+1); if(out) { memcpy(out,bytes.bytes,*length); out[*length]=0; } return out;
}
void rp_free_text(char *text) { free(text); }
void rp_state(const char *title,const char *path,const char *status,int dirty,int working,int readonly) {
    window.title=S(title); window.documentEdited=dirty; statusLabel.stringValue=S(status);
    NSString *file=S(path); NSURL *url=file.length ? [NSURL fileURLWithPath:file] : nil;
    if (![window.representedURL isEqual:url]) { window.representedURL=url; if(url) { [NSDocumentController.sharedDocumentController noteNewRecentDocumentURL:url]; rp_rebuild_menus(); } }
    busy=working; readonlyDocument=readonly; editor.editable=!working&&!readonly; filePosition.hidden=!readonly; filePosition.enabled=!working;
}
void rp_preferences(const char *font,double points,int spell,int language) {
    NSString *name=S(font);
    if(![currentFont isEqualToString:name] || currentSize!=points) {
        currentFont=name; currentSize=points;
        editor.font=[NSFont fontWithName:name size:points] ?: [NSFont monospacedSystemFontOfSize:points weight:NSFontWeightRegular];
    }
    currentSpell=spell; currentLanguage=language;
    editor.continuousSpellCheckingEnabled=spell;
    if(delegate.settings.visible) {
        [delegate.languages selectItemAtIndex:language]; delegate.spelling.state=spell;
    }
}
void rp_close(void) {
    [window orderOut:nil]; [NSApp stop:nil];
    [NSApp postEvent:[NSEvent otherEventWithType:NSEventTypeApplicationDefined location:NSZeroPoint modifierFlags:0 timestamp:0 windowNumber:0 context:nil subtype:0 data1:0 data2:0] atStart:NO];
}

int rp_smoke_test(void) {
    @autoreleasepool {
        smokeTest=YES; [NSApplication sharedApplication]; delegate=[RavnDelegate new];
        [delegate applicationDidFinishLaunching:[NSNotification notificationWithName:NSApplicationDidFinishLaunchingNotification object:NSApp]];
        const char *sample="Native UTF-8: æøå 日本語 😀\nSecond line";
        rp_document(sample,strlen(sample),0);
        size_t length=0; char *copy=rp_copy_text(&length);
        BOOL valid=copy && length==strlen(sample) && memcmp(copy,sample,length)==0;
        rp_free_text(copy);
        [editor.undoManager beginUndoGrouping];
        [editor insertText:@"!" replacementRange:NSMakeRange(editor.string.length,0)];
        [editor.undoManager endUndoGrouping];
        valid=valid && [editor.string hasSuffix:@"!"] && editor.undoManager.canUndo;
        [editor.undoManager undo];
        valid=valid && [editor.string isEqualToString:S(sample)];
        [editor.undoManager redo]; valid=valid && [editor.string hasSuffix:@"!"];
        rp_document("next",4,1);
        valid=valid && !editor.editable && !editor.undoManager.canUndo && [editor.string isEqualToString:@"next"];
        fprintf(stderr,"Native AppKit smoke test: %s\n",valid?"PASS":"FAIL");
        [window close]; return valid?0:1;
    }
}
