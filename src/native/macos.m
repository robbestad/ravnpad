#import <AppKit/AppKit.h>
#include "bridge.h"
#include <stdlib.h>
#include <string.h>

static NSWindow *window;
static NSScrollView *scroll;
static NSTextView *editor;
static NSTextField *statusLabel;
static NSSlider *filePosition;
static BOOL updating, busy, readonlyDocument, largeDocument, smokeTest, terminationPending;
static NSString *largeQuery;
static BOOL largeMatchCase, largeWholeWord;
static NSString *currentFont;
static double currentSize;
static BOOL currentSpell;
static int currentLanguage;
static int currentWrap=-1;
static int currentTheme=-1;

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
@property NSButton *wrapping;
@property NSPopUpButton *themes;
- (void)command:(NSMenuItem *)sender;
- (void)showSettings:(id)sender;
- (void)findDocument:(NSMenuItem *)sender;
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
    command(file, RP_NEW, @"n"); command(file, RP_NEW_WINDOW, @"N"); command(file, RP_OPEN, @"o");
    NSMenu *recent = submenu(file, L(RP_RECENT));
    NSArray<NSURL *> *recentURLs=NSDocumentController.sharedDocumentController.recentDocumentURLs;
    NSCountedSet *recentNames=[NSCountedSet setWithArray:[recentURLs valueForKey:@"lastPathComponent"]];
    for (NSURL *url in recentURLs) {
        NSString *title=url.lastPathComponent;
        if([recentNames countForObject:title]>1) title=[NSString stringWithFormat:@"%@ — %@",title,url.URLByDeletingLastPathComponent.path];
        NSMenuItem *entry = item(recent, title, @selector(openRecent:), @"", delegate, 0);
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
    item(edit,L(RP_FIND),@selector(findDocument:),@"f",delegate,NSTextFinderActionShowFindInterface);
    item(edit,L(RP_REPLACE),@selector(performTextFinderAction:),@"",nil,NSTextFinderActionShowReplaceInterface);
    item(edit,L(RP_NEXT),@selector(findDocument:),@"g",delegate,NSTextFinderActionNextMatch);
    item(edit,L(RP_PREVIOUS),@selector(findDocument:),@"G",delegate,NSTextFinderActionPreviousMatch);
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
    window.releasedWhenClosed=NO;
    if(smokeTest) [window setContentSize:NSMakeSize(1100,780)];
    else [window setFrameAutosaveName:@"RavnPadDocument"];
    NSView *content=window.contentView;
    // Autosave may restore a different size before the subviews are created.
    // Autoresizing preserves their initial margins, so derive those from the
    // actual content bounds rather than the default window dimensions.
    NSSize size=content.bounds.size;
    scroll=[[NSScrollView alloc] initWithFrame:NSMakeRect(0,28,size.width,size.height-28)];
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
    statusLabel.frame=NSMakeRect(12,5,size.width-30,18); statusLabel.autoresizingMask=NSViewWidthSizable;
    statusLabel.font=[NSFont systemFontOfSize:NSFont.smallSystemFontSize]; statusLabel.textColor=NSColor.secondaryLabelColor;
    [content addSubview:statusLabel];
    filePosition=[[NSSlider alloc] initWithFrame:NSMakeRect(size.width-250,3,230,22)];
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
- (void)findDocument:(NSMenuItem *)sender {
    if(!largeDocument) { [editor performTextFinderAction:sender]; return; }
    if(sender.tag==NSTextFinderActionShowFindInterface || !largeQuery.length) {
        NSAlert *alert=[NSAlert new]; alert.messageText=L(RP_FIND);
        [alert addButtonWithTitle:L(RP_FIND)]; [alert addButtonWithTitle:L(RP_CANCEL)];
        NSView *options=[[NSView alloc] initWithFrame:NSMakeRect(0,0,360,82)];
        NSTextField *field=[[NSTextField alloc] initWithFrame:NSMakeRect(0,52,360,24)]; field.stringValue=largeQuery ?: @""; [options addSubview:field];
        NSButton *matchCase=[NSButton checkboxWithTitle:@"Aa" target:nil action:nil]; matchCase.frame=NSMakeRect(0,24,160,22); matchCase.state=largeMatchCase; [options addSubview:matchCase];
        NSButton *wholeWord=[NSButton checkboxWithTitle:L(RP_WHOLE_WORD) target:nil action:nil]; wholeWord.frame=NSMakeRect(170,24,190,22); wholeWord.state=largeWholeWord; [options addSubview:wholeWord];
        alert.accessoryView=options; [alert.window makeFirstResponder:field];
        if([alert runModal]!=NSAlertFirstButtonReturn || !field.stringValue.length) return;
        largeQuery=field.stringValue; largeMatchCase=matchCase.state==NSControlStateValueOn; largeWholeWord=wholeWord.state==NSControlStateValueOn;
    }
    BOOL backwards=sender.tag==NSTextFinderActionPreviousMatch;
    if(rp_find_large(largeQuery.UTF8String,backwards,largeMatchCase,largeWholeWord)) rp_tick();
}
- (void)moveFile:(NSSlider *)sender { rp_view(sender.doubleValue); }
- (BOOL)windowShouldClose:(NSWindow *)sender { (void)sender; rp_action(RP_QUIT); return NO; }
- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)sender {
    (void)sender; if(busy) return NSTerminateCancel;
    terminationPending=YES; rp_action(RP_QUIT);
    dispatch_async(dispatch_get_main_queue(), ^{ rp_tick(); });
    return NSTerminateLater;
}
- (void)application:(NSApplication *)sender openFiles:(NSArray<NSString *> *)files {
    for (NSString *path in files) rp_open(path.UTF8String);
    [sender replyToOpenOrPrint:NSApplicationDelegateReplySuccess];
}
- (BOOL)validateMenuItem:(NSMenuItem *)item {
    if (item.action==@selector(command:)) return !busy && (!(item.tag==RP_SAVE || item.tag==RP_SAVE_AS) || !readonlyDocument);
    if (item.action==@selector(findDocument:)) return !busy;
    if (largeDocument && item.action==@selector(performTextFinderAction:)) return NO;
    return YES;
}
- (void)changeLanguage:(NSPopUpButton *)sender { rp_action(100+(int)sender.indexOfSelectedItem); rp_tick(); }
- (void)changeSpelling:(NSButton *)sender { (void)sender; rp_action(RP_SPELL); rp_tick(); }
- (void)changeWrapping:(NSButton *)sender { (void)sender; rp_action(RP_WRAP); rp_tick(); }
- (void)changeTheme:(NSPopUpButton *)sender { rp_action(RP_THEME_SYSTEM+(int)sender.indexOfSelectedItem); rp_tick(); }
- (void)showFont:(id)sender { (void)sender; [window makeFirstResponder:editor]; [[NSFontManager sharedFontManager] setSelectedFont:editor.font isMultiple:NO]; [[NSFontManager sharedFontManager] orderFrontFontPanel:self]; }
- (void)showSettings:(id)sender {
    (void)sender;
    self.settings=[[NSPanel alloc] initWithContentRect:NSMakeRect(0,0,420,250) styleMask:NSWindowStyleMaskTitled|NSWindowStyleMaskClosable backing:NSBackingStoreBuffered defer:NO];
    self.settings.title=L(RP_SETTINGS); self.settings.releasedWhenClosed=NO;
    NSTextField *label=[NSTextField labelWithString:L(RP_LANGUAGE)]; label.frame=NSMakeRect(20,198,140,24); [self.settings.contentView addSubview:label];
    self.languages=[[NSPopUpButton alloc] initWithFrame:NSMakeRect(165,198,230,26) pullsDown:NO];
    for(int i=0;i<15;i++) [self.languages addItemWithTitle:L(100+i)];
    [self.languages selectItemAtIndex:currentLanguage];
    self.languages.target=self; self.languages.action=@selector(changeLanguage:); [self.settings.contentView addSubview:self.languages];
    NSTextField *themeLabel=[NSTextField labelWithString:L(RP_THEME)]; themeLabel.frame=NSMakeRect(20,153,140,24); [self.settings.contentView addSubview:themeLabel];
    self.themes=[[NSPopUpButton alloc] initWithFrame:NSMakeRect(165,153,230,26) pullsDown:NO];
    [self.themes addItemWithTitle:L(RP_THEME_SYSTEM)]; [self.themes addItemWithTitle:L(RP_THEME_LIGHT)]; [self.themes addItemWithTitle:L(RP_THEME_DARK)];
    [self.themes selectItemAtIndex:currentTheme]; self.themes.target=self; self.themes.action=@selector(changeTheme:); [self.settings.contentView addSubview:self.themes];
    NSButton *font=[NSButton buttonWithTitle:L(RP_FONT) target:self action:@selector(showFont:)]; font.frame=NSMakeRect(20,100,170,32); [self.settings.contentView addSubview:font];
    self.spelling=[NSButton checkboxWithTitle:L(RP_SPELL) target:self action:@selector(changeSpelling:)]; self.spelling.frame=NSMakeRect(20,57,370,26); self.spelling.state=currentSpell; [self.settings.contentView addSubview:self.spelling];
    self.wrapping=[NSButton checkboxWithTitle:L(RP_WRAP) target:self action:@selector(changeWrapping:)]; self.wrapping.frame=NSMakeRect(20,22,370,26); self.wrapping.state=editor.textContainer.widthTracksTextView; [self.settings.contentView addSubview:self.wrapping];
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
void rp_replace_text(const char *text,size_t length) {
    NSString *value=[[NSString alloc] initWithBytes:text length:length encoding:NSUTF8StringEncoding];
    if(!value || !editor.editable) return;
    updating=YES;
    [editor.undoManager beginUndoGrouping];
    [editor insertText:value replacementRange:NSMakeRange(0,editor.string.length)];
    [editor.undoManager endUndoGrouping];
    updating=NO;
}
char *rp_copy_text(size_t *length) {
    NSData *bytes=[editor.string dataUsingEncoding:NSUTF8StringEncoding]; *length=bytes.length;
    char *out=malloc(*length+1); if(out) { memcpy(out,bytes.bytes,*length); out[*length]=0; } return out;
}
void rp_free_text(char *text) { free(text); }
void rp_state(const char *title,const char *path,const char *status,int dirty,int working,int readonly,int large) {
    window.title=S(title); window.documentEdited=dirty; statusLabel.stringValue=S(status);
    NSString *file=S(path); NSURL *url=file.length ? [NSURL fileURLWithPath:file] : nil;
    if (![window.representedURL isEqual:url]) { window.representedURL=url; if(url) { [NSDocumentController.sharedDocumentController noteNewRecentDocumentURL:url]; rp_rebuild_menus(); } }
    busy=working; readonlyDocument=readonly; largeDocument=large; editor.editable=!working&&!readonly; filePosition.hidden=!readonly; filePosition.enabled=!working;
}
void rp_find_result(size_t length) {
    if(!length) { NSAlert *alert=[NSAlert new]; alert.messageText=L(RP_NO_MATCHES); [alert runModal]; return; }
    [editor setSelectedRange:NSMakeRange(0,length)]; [editor scrollRangeToVisible:NSMakeRange(0,length)];
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
void rp_wrap(int enabled) {
    BOOL wrap=enabled!=0;
    if(currentWrap==wrap) return;
    currentWrap=wrap;
    scroll.hasHorizontalScroller=!wrap;
    editor.horizontallyResizable=!wrap;
    editor.textContainer.widthTracksTextView=wrap;
    editor.textContainer.containerSize=NSMakeSize(wrap?scroll.contentSize.width:CGFLOAT_MAX,CGFLOAT_MAX);
    if(delegate.settings.visible) delegate.wrapping.state=wrap;
}
void rp_theme(int preference) {
    if(currentTheme==preference) return;
    currentTheme=preference;
    NSAppearance *appearance=nil;
    if(preference==1) appearance=[NSAppearance appearanceNamed:NSAppearanceNameAqua];
    else if(preference==2) appearance=[NSAppearance appearanceNamed:NSAppearanceNameDarkAqua];
    NSApp.appearance=appearance;
    if(delegate.settings.visible) [delegate.themes selectItemAtIndex:preference];
}
double rp_read_position(void) {
    [editor.layoutManager ensureLayoutForTextContainer:editor.textContainer];
    CGFloat maximum=MAX(0,editor.bounds.size.height-scroll.contentView.bounds.size.height);
    return maximum>0?scroll.contentView.bounds.origin.y/maximum:0;
}
void rp_restore_position(double fraction) {
    [editor.layoutManager ensureLayoutForTextContainer:editor.textContainer];
    CGFloat maximum=MAX(0,editor.bounds.size.height-scroll.contentView.bounds.size.height);
    NSPoint point=scroll.contentView.bounds.origin; point.y=maximum*MAX(0,MIN(1,fraction));
    [scroll.contentView scrollToPoint:point]; [scroll reflectScrolledClipView:scroll.contentView];
}
void rp_close(void) {
    if(terminationPending) { terminationPending=NO; [NSApp replyToApplicationShouldTerminate:YES]; return; }
    [window orderOut:nil]; [NSApp stop:nil];
    [NSApp postEvent:[NSEvent otherEventWithType:NSEventTypeApplicationDefined location:NSZeroPoint modifierFlags:0 timestamp:0 windowNumber:0 context:nil subtype:0 data1:0 data2:0] atStart:NO];
}

static BOOL layoutIsValid(void) {
    [window.contentView layoutSubtreeIfNeeded];
    NSSize size=window.contentView.bounds.size;
    NSRect frame=scroll.frame;
    return fabs(frame.origin.x)<0.5 && fabs(frame.origin.y-28)<0.5
        && fabs(frame.size.width-size.width)<0.5
        && fabs(NSMaxY(frame)-size.height)<0.5
        && fabs(NSMaxX(statusLabel.frame)-(size.width-18))<0.5
        && fabs(NSMaxX(filePosition.frame)-(size.width-20))<0.5;
}

int rp_smoke_test(void) {
    @autoreleasepool {
        smokeTest=YES; [NSApplication sharedApplication]; delegate=[RavnDelegate new];
        [delegate applicationDidFinishLaunching:[NSNotification notificationWithName:NSApplicationDidFinishLaunchingNotification object:NSApp]];
        const char *sample="Native UTF-8: æøå 日本語 😀\nSecond line";
        rp_document(sample,strlen(sample),0);
        size_t length=0; char *copy=rp_copy_text(&length);
        BOOL valid=layoutIsValid() && copy && length==strlen(sample) && memcmp(copy,sample,length)==0;
        rp_free_text(copy);
        [window setContentSize:NSMakeSize(420,300)];
        valid=layoutIsValid() && valid;
        [window setContentSize:NSMakeSize(1300,900)];
        valid=layoutIsValid() && valid;
        [editor.undoManager beginUndoGrouping];
        [editor insertText:@"!" replacementRange:NSMakeRange(editor.string.length,0)];
        [editor.undoManager endUndoGrouping];
        valid=valid && [editor.string hasSuffix:@"!"] && editor.undoManager.canUndo;
        [editor.undoManager undo];
        valid=valid && [editor.string isEqualToString:S(sample)];
        [editor.undoManager redo]; valid=valid && [editor.string hasSuffix:@"!"];
        rp_replace_text("agent",5); valid=valid && [editor.string isEqualToString:@"agent"];
        [editor.undoManager undo]; valid=valid && [editor.string hasSuffix:@"!"];
        [editor.undoManager undo]; valid=valid && [editor.string isEqualToString:S(sample)];
        rp_document("next",4,1);
        valid=valid && !editor.editable && !editor.undoManager.canUndo && [editor.string isEqualToString:@"next"];
        fprintf(stderr,"Native AppKit smoke test: %s\n",valid?"PASS":"FAIL");
        [window close]; return valid?0:1;
    }
}

void rp_lock(void) { busy=YES; editor.editable=NO; }

void rp_cancel_close(void) { if(terminationPending) { terminationPending=NO; [NSApp replyToApplicationShouldTerminate:NO]; } }

int rp_confirm(const char *title,const char *body,const char *accept,const char *discard,const char *cancel) {
    NSAlert *alert=[NSAlert new];
    alert.alertStyle=NSAlertStyleWarning;
    alert.messageText=S(title);
    alert.informativeText=S(body);
    [alert addButtonWithTitle:S(accept)];
    [alert addButtonWithTitle:S(discard)];
    [alert addButtonWithTitle:S(cancel)];
    // NSAlert creates its window lazily. Assign the document window's effective
    // appearance after adding the buttons so forced light/dark themes are kept.
    alert.window.appearance=window.effectiveAppearance;
    NSModalResponse response=[alert runModal];
    if(response==NSAlertFirstButtonReturn) return 1;
    if(response==NSAlertSecondButtonReturn) return 2;
    return 0;
}
