#define _WIN32_WINNT 0x0A00
#define _RICHEDIT_VER 0x0800
#include <windows.h>
#include <commctrl.h>
#include <commdlg.h>
#include <richedit.h>
#include <dwmapi.h>
#include <shellapi.h>
#include <ole2.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>
#include <limits.h>
#include <stdio.h>
#include "bridge.h"

static HWND window, editor, status, position, findDialog;
static HMENU menu, themeMenu;
static HFONT font;
static HBRUSH themeBrush;
static HACCEL accelerators;
static UINT findMessage;
static FINDREPLACEW find;
static DWORD findOptions=FR_DOWN;
static wchar_t query[1024], replacement[1024], currentFont[LF_FACESIZE];
static double currentSize;
static int updating, busy, readonlyDocument, currentSpell, currentWrap=-1, currentTheme=-1, darkTheme, smokeTest, smokeResult, documentDirty;
static int wheelRemainder;
static LONG selectionAnchor=-1;
static UINT dpi=96;

static wchar_t *wide(const char *s) {
    int count=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,s,-1,NULL,0);
    if(!count) return NULL;
    wchar_t *out=calloc((size_t)count,sizeof(wchar_t));
    if(out) MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,s,-1,out,count);
    return out;
}
static char *utf8(const wchar_t *s) {
    int count=WideCharToMultiByte(CP_UTF8,WC_ERR_INVALID_CHARS,s,-1,NULL,0,NULL,NULL);
    if(!count) return NULL;
    char *out=malloc((size_t)count);
    if(out) WideCharToMultiByte(CP_UTF8,WC_ERR_INVALID_CHARS,s,-1,out,count,NULL,NULL);
    return out;
}
static void text(HWND control,const char *s) { wchar_t *w=wide(s); if(w) { SetWindowTextW(control,w); free(w); } }
static void entry(HMENU parent,int id,const wchar_t *shortcut) {
    wchar_t *label=wide(rp_label(id)); if(!label) return;
    size_t length=wcslen(label)+(shortcut?wcslen(shortcut):0)+2;
    wchar_t *value=calloc(length,sizeof(wchar_t));
    if(value) { wcscpy_s(value,length,label); if(shortcut) wcscat_s(value,length,shortcut); AppendMenuW(parent,MF_STRING,id,value); free(value); }
    free(label);
}
static HMENU submenu(HMENU parent,int id) {
    HMENU child=CreatePopupMenu(); wchar_t *label=wide(rp_label(id));
    AppendMenuW(parent,MF_POPUP,(UINT_PTR)child,label?label:L""); free(label); return child;
}
static int system_dark(void) {
    DWORD light=1,size=sizeof(light);
    LSTATUS result=RegGetValueW(HKEY_CURRENT_USER,L"Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",L"AppsUseLightTheme",RRF_RT_REG_DWORD,NULL,&light,&size);
    return result==ERROR_SUCCESS && light==0;
}
static void apply_window_chrome(void) {
    if(!window) return;
    BOOL enabled=darkTheme?TRUE:FALSE;
    // DWMWA_USE_IMMERSIVE_DARK_MODE is 20 on current Windows 10/11 builds.
    // Build 17763 used 19, so retain the fallback for older supported systems.
    HRESULT result=DwmSetWindowAttribute(window,20,&enabled,sizeof(enabled));
    if(FAILED(result)) DwmSetWindowAttribute(window,19,&enabled,sizeof(enabled));
    const wchar_t *controlTheme=darkTheme?L"DarkMode_Explorer":L"Explorer";
    if(editor) SetWindowTheme(editor,controlTheme,NULL);
    if(status) {
        SetWindowTheme(status,controlTheme,NULL);
        SendMessageW(status,SB_SETBKCOLOR,0,darkTheme?RGB(32,32,32):GetSysColor(COLOR_BTNFACE));
    }
    if(position) SetWindowTheme(position,controlTheme,NULL);
    RedrawWindow(window,NULL,NULL,RDW_INVALIDATE|RDW_ERASE|RDW_FRAME|RDW_ALLCHILDREN);
}
static void apply_colors(void) {
    darkTheme=currentTheme==2||(currentTheme==0&&system_dark());
    COLORREF background=darkTheme?RGB(30,30,30):GetSysColor(COLOR_WINDOW);
    COLORREF foreground=darkTheme?RGB(235,235,235):GetSysColor(COLOR_WINDOWTEXT);
    SendMessageW(editor,EM_SETBKGNDCOLOR,0,background);
    CHARFORMAT2W format={0}; format.cbSize=sizeof(format); format.dwMask=CFM_COLOR; format.crTextColor=foreground;
    SendMessageW(editor,EM_SETCHARFORMAT,SCF_ALL,(LPARAM)&format);
    if(themeBrush) DeleteObject(themeBrush); themeBrush=CreateSolidBrush(darkTheme?RGB(32,32,32):GetSysColor(COLOR_BTNFACE));
    apply_window_chrome();
}
void rp_rebuild_menus(void) {
    HMENU previous=menu; menu=CreateMenu();
    HMENU file=submenu(menu,RP_FILE);
    entry(file,RP_NEW,L"\tCtrl+N"); entry(file,RP_NEW_WINDOW,L"\tCtrl+Shift+N"); entry(file,RP_OPEN,L"\tCtrl+O");
    HMENU recent=submenu(file,RP_RECENT); for(int i=0;i<10&&rp_label(200+i)[0];i++) entry(recent,200+i,NULL);
    entry(file,RP_SAVE,L"\tCtrl+S"); entry(file,RP_SAVE_AS,L"\tCtrl+Shift+S");
    AppendMenuW(file,MF_SEPARATOR,0,NULL); entry(file,RP_RECOVERY,NULL); entry(file,RP_QUIT,L"\tAlt+F4");
    HMENU edit=submenu(menu,RP_EDIT);
    entry(edit,RP_UNDO,L"\tCtrl+Z"); entry(edit,RP_REDO,L"\tCtrl+Y"); AppendMenuW(edit,MF_SEPARATOR,0,NULL);
    entry(edit,RP_CUT,L"\tCtrl+X"); entry(edit,RP_COPY,L"\tCtrl+C"); entry(edit,RP_PASTE,L"\tCtrl+V"); entry(edit,RP_SELECT_ALL,L"\tCtrl+A");
    AppendMenuW(edit,MF_SEPARATOR,0,NULL); entry(edit,RP_FIND,L"\tCtrl+F"); entry(edit,RP_REPLACE,L"\tCtrl+H"); entry(edit,RP_NEXT,L"\tF3");
    HMENU settings=submenu(menu,RP_SETTINGS); entry(settings,RP_FONT,NULL); entry(settings,RP_SPELL,NULL); entry(settings,RP_WRAP,NULL);
    CheckMenuItem(settings,RP_WRAP,MF_BYCOMMAND|((currentWrap==1)?MF_CHECKED:MF_UNCHECKED));
    themeMenu=submenu(settings,RP_THEME); entry(themeMenu,RP_THEME_SYSTEM,NULL); entry(themeMenu,RP_THEME_LIGHT,NULL); entry(themeMenu,RP_THEME_DARK,NULL);
    if(currentTheme>=0) CheckMenuRadioItem(themeMenu,RP_THEME_SYSTEM,RP_THEME_DARK,RP_THEME_SYSTEM+currentTheme,MF_BYCOMMAND);
    HMENU languages=submenu(settings,RP_LANGUAGE); for(int i=0;i<15;i++) entry(languages,100+i,NULL);
    HMENU help=submenu(menu,RP_HELP); entry(help,RP_UPDATE,NULL); entry(help,RP_ABOUT,NULL);
    SetMenu(window,menu); DrawMenuBar(window); if(previous) DestroyMenu(previous);
}
static void resize(void) {
    RECT rect; GetClientRect(window,&rect); int bar=MulDiv(28,dpi,96);
    MoveWindow(editor,0,0,rect.right,rect.bottom-bar,TRUE);
    MoveWindow(status,0,rect.bottom-bar,rect.right,bar,TRUE);
    MoveWindow(position,rect.right-MulDiv(230,dpi,96),rect.bottom-bar, MulDiv(220,dpi,96),bar,TRUE);
    RECT textRect; GetClientRect(editor,&textRect);
    int margin=MulDiv(12,dpi,96);
    // Rich Edit does not include a deflated bottom edge in its maximum scroll
    // position. Keeping a bottom inset therefore leaves the final visual row
    // partially clipped when the scrollbar reaches the end. Retain the top
    // and side padding, but let the formatting rectangle reach the viewport's
    // bottom edge.
    textRect.left+=margin; textRect.right-=margin; textRect.top+=margin;
    SendMessageW(editor,EM_SETRECT,0,(LPARAM)&textRect);
}
static void scroll_wheel(WPARAM value) {
    UINT lines=3;
    SystemParametersInfoW(SPI_GETWHEELSCROLLLINES,0,&lines,0);
    wheelRemainder+=(short)HIWORD(value);
    int notches=wheelRemainder/WHEEL_DELTA;
    wheelRemainder-=notches*WHEEL_DELTA;
    if(!notches) return;
    if(lines==WHEEL_PAGESCROLL) {
        UINT action=notches>0?SB_PAGEUP:SB_PAGEDOWN;
        for(int i=0;i<abs(notches);i++) SendMessageW(editor,EM_SCROLL,action,0);
    } else {
        SendMessageW(editor,EM_LINESCROLL,0,-notches*(int)lines);
    }
}
static void command(int id);
static int editor_command_enabled(int id) {
    CHARRANGE selection={0};
    SendMessageW(editor,EM_EXGETSEL,0,(LPARAM)&selection);
    int hasSelection=selection.cpMin!=selection.cpMax;
    switch(id) {
        case RP_UNDO: return !readonlyDocument&&!busy&&SendMessageW(editor,EM_CANUNDO,0,0);
        case RP_REDO: return !readonlyDocument&&!busy&&SendMessageW(editor,EM_CANREDO,0,0);
        case RP_CUT: return hasSelection&&!readonlyDocument&&!busy;
        case RP_COPY: return hasSelection;
        case RP_PASTE: return !readonlyDocument&&!busy
            &&(IsClipboardFormatAvailable(CF_UNICODETEXT)||IsClipboardFormatAvailable(CF_TEXT));
        case RP_SELECT_ALL: return GetWindowTextLengthW(editor)>0;
        default: return 0;
    }
}
static void context_entry(HMENU popup,int id) {
    wchar_t *label=wide(rp_label(id));
    AppendMenuW(popup,MF_STRING|(editor_command_enabled(id)?MF_ENABLED:MF_GRAYED),id,label?label:L"");
    free(label);
}
static void show_editor_context_menu(LPARAM coordinates) {
    POINT point={0};
    if((INT_PTR)coordinates==-1) {
        if(!GetCaretPos(&point)) {
            RECT rect={0}; GetClientRect(editor,&rect);
            point.x=rect.left+MulDiv(24,dpi,96); point.y=rect.top+MulDiv(24,dpi,96);
        }
        ClientToScreen(editor,&point);
    } else {
        point.x=(short)LOWORD(coordinates); point.y=(short)HIWORD(coordinates);
    }
    HMENU popup=CreatePopupMenu();
    if(!popup) return;
    context_entry(popup,RP_UNDO); context_entry(popup,RP_REDO);
    AppendMenuW(popup,MF_SEPARATOR,0,NULL);
    context_entry(popup,RP_CUT); context_entry(popup,RP_COPY); context_entry(popup,RP_PASTE);
    AppendMenuW(popup,MF_SEPARATOR,0,NULL); context_entry(popup,RP_SELECT_ALL);
    SetForegroundWindow(window);
    int selected=TrackPopupMenu(popup,TPM_RETURNCMD|TPM_RIGHTBUTTON,point.x,point.y,0,window,NULL);
    DestroyMenu(popup);
    if(selected) command(selected);
    PostMessageW(window,WM_NULL,0,0);
}
static LONG editor_character_at(LPARAM coordinates) {
    POINTL point={(short)LOWORD(coordinates),(short)HIWORD(coordinates)};
    LRESULT character=SendMessageW(editor,EM_CHARFROMPOS,0,(LPARAM)&point);
    LONG length=GetWindowTextLengthW(editor);
    if(character<0) return 0;
    return character>length?length:(LONG)character;
}
static void extend_selection_to(LONG target) {
    CHARRANGE selection={0};
    SendMessageW(editor,EM_EXGETSEL,0,(LPARAM)&selection);
    LONG anchor=selection.cpMin==selection.cpMax?selection.cpMin:selectionAnchor;
    if(anchor<0) anchor=selection.cpMin;
    CHARRANGE extended={anchor<target?anchor:target,anchor<target?target:anchor};
    SetFocus(editor);
    SendMessageW(editor,EM_EXSETSEL,0,(LPARAM)&extended);
    SendMessageW(editor,EM_SCROLLCARET,0,0);
    selectionAnchor=anchor;
}
static LRESULT CALLBACK editor_procedure(HWND hwnd,UINT message,WPARAM w,LPARAM l,UINT_PTR id,DWORD_PTR data) {
    (void)data;
    if(message==WM_MOUSEWHEEL) { scroll_wheel(w); return 0; }
    if(message==WM_CONTEXTMENU) { show_editor_context_menu(l); return 0; }
    if(message==WM_LBUTTONDOWN) {
        LONG target=editor_character_at(l);
        if(w&MK_SHIFT) { extend_selection_to(target); return 0; }
        selectionAnchor=target;
    }
    if(message==WM_NCDESTROY) RemoveWindowSubclass(hwnd,editor_procedure,id);
    return DefSubclassProc(hwnd,message,w,l);
}
static void choose_font(void) {
    LOGFONTW lf={0}; if(font) GetObjectW(font,sizeof(lf),&lf);
    CHOOSEFONTW choice={0}; choice.lStructSize=sizeof(choice); choice.hwndOwner=window;
    choice.lpLogFont=&lf; choice.Flags=CF_SCREENFONTS|CF_INITTOLOGFONTSTRUCT|CF_FORCEFONTEXIST;
    if(ChooseFontW(&choice)) { char *name=utf8(lf.lfFaceName); if(name) { rp_font(name,choice.iPointSize/10.0); free(name); rp_tick(); } }
}
static void show_find(int replace) {
    if(findDialog) { SetForegroundWindow(findDialog); return; }
    ZeroMemory(&find,sizeof(find)); find.lStructSize=sizeof(find); find.hwndOwner=window;
    find.Flags=findOptions; find.lpstrFindWhat=query; find.wFindWhatLen=1024;
    find.lpstrReplaceWith=replacement; find.wReplaceWithLen=1024;
    findDialog=replace?ReplaceTextW(&find):FindTextW(&find);
}
static int find_next(int wrap,int notify) {
    if(!query[0]) { show_find(0); return 0; }
    int down=(find.Flags&FR_DOWN)!=0;
    char *largeQuery=utf8(query);
    if(largeQuery&&rp_find_large(largeQuery,!down,(find.Flags&FR_MATCHCASE)!=0,(find.Flags&FR_WHOLEWORD)!=0)) {
        free(largeQuery); rp_tick(); return 1;
    }
    free(largeQuery);
    CHARRANGE selection; SendMessageW(editor,EM_EXGETSEL,0,(LPARAM)&selection);
    FINDTEXTEXW search={0}; search.lpstrText=query;
    search.chrg.cpMin=down?selection.cpMax:selection.cpMin;
    search.chrg.cpMax=down?-1:0;
    WPARAM flags=find.Flags&(FR_DOWN|FR_MATCHCASE|FR_WHOLEWORD);
    LRESULT result=SendMessageW(editor,EM_FINDTEXTEXW,flags,(LPARAM)&search);
    if(result==-1 && wrap) {
        search.chrg.cpMin=down?0:GetWindowTextLengthW(editor); search.chrg.cpMax=down?-1:0;
        result=SendMessageW(editor,EM_FINDTEXTEXW,flags,(LPARAM)&search);
    }
    if(result==-1) {
        if(notify) { wchar_t *message=wide(rp_label(RP_NO_MATCHES)); MessageBoxW(window,message?message:L"No matches found",L"RavnPad",MB_OK|MB_ICONINFORMATION); free(message); }
        return 0;
    }
    SendMessageW(editor,EM_EXSETSEL,0,(LPARAM)&search.chrgText); SendMessageW(editor,EM_SCROLLCARET,0,0); return 1;
}
static void replace_selection(void) {
    if(readonlyDocument||busy) return;
    CHARRANGE selected; SendMessageW(editor,EM_EXGETSEL,0,(LPARAM)&selected);
    if(selected.cpMax-selected.cpMin!=(LONG)wcslen(query)) return;
    wchar_t *value=calloc(wcslen(query)+1,sizeof(wchar_t)); if(!value)return;
    SendMessageW(editor,EM_GETSELTEXT,0,(LPARAM)value);
    int match=(find.Flags&FR_MATCHCASE)?wcscmp(value,query)==0:_wcsicmp(value,query)==0;
    free(value); if(match) SendMessageW(editor,EM_REPLACESEL,TRUE,(LPARAM)replacement);
}
static void command(int id) {
    switch(id) {
        case RP_FONT: choose_font(); return;
        case RP_FIND: show_find(0); return;
        case RP_REPLACE: if(!readonlyDocument) show_find(1); return;
        case RP_NEXT: find_next(1,1); return;
        case RP_UNDO: SendMessageW(editor,EM_UNDO,0,0); return;
        case RP_REDO: SendMessageW(editor,EM_REDO,0,0); return;
        case RP_CUT: SendMessageW(editor,WM_CUT,0,0); return;
        case RP_COPY: SendMessageW(editor,WM_COPY,0,0); return;
        case RP_PASTE: SendMessageW(editor,WM_PASTE,0,0); return;
        case RP_SELECT_ALL: { CHARRANGE all={0,-1}; SendMessageW(editor,EM_EXSETSEL,0,(LPARAM)&all); return; }
        default: rp_action(id); rp_tick(); return;
    }
}
static LRESULT CALLBACK procedure(HWND hwnd,UINT message,WPARAM w,LPARAM l) {
    if(message==findMessage && findMessage) {
        FINDREPLACEW *event=(FINDREPLACEW *)l;
        findOptions=event->Flags&(FR_DOWN|FR_MATCHCASE|FR_WHOLEWORD);
        if(event->Flags&FR_DIALOGTERM) findDialog=NULL;
        else if(event->Flags&FR_FINDNEXT) find_next(1,1);
        else if(event->Flags&FR_REPLACE) { replace_selection(); find_next(1,1); }
        else if((event->Flags&FR_REPLACEALL)&&!readonlyDocument&&!busy&&query[0]) {
            CHARRANGE start={0,0}; SendMessageW(editor,EM_EXSETSEL,0,(LPARAM)&start); find.Flags|=FR_DOWN;
            SendMessageW(editor,WM_SETREDRAW,FALSE,0);
            while(find_next(0,0)) replace_selection();
            SendMessageW(editor,WM_SETREDRAW,TRUE,0); InvalidateRect(editor,NULL,TRUE);
        }
        return 0;
    }
    switch(message) {
        case WM_CREATE: {
            window=hwnd; dpi=GetDpiForWindow(hwnd);
            editor=CreateWindowExW(0,MSFTEDIT_CLASS,L"",WS_CHILD|WS_VISIBLE|WS_VSCROLL|WS_HSCROLL|ES_MULTILINE|ES_AUTOVSCROLL|ES_AUTOHSCROLL|ES_WANTRETURN|ES_NOHIDESEL,0,0,0,0,hwnd,(HMENU)1,GetModuleHandleW(NULL),NULL);
            if(!editor) return -1;
            SetWindowSubclass(editor,editor_procedure,1,0);
            SendMessageW(editor,EM_SETTEXTMODE,TM_PLAINTEXT|TM_MULTILEVELUNDO,0);
            SendMessageW(editor,EM_EXLIMITTEXT,0,0x7ffffffe);
            SendMessageW(editor,EM_SETEVENTMASK,0,ENM_CHANGE);
            SendMessageW(editor,EM_SETEDITSTYLE,SES_USECTF,SES_USECTF);
            SendMessageW(editor,EM_SETBKGNDCOLOR,0,GetSysColor(COLOR_WINDOW));
            status=CreateWindowExW(0,STATUSCLASSNAMEW,L"",WS_CHILD|WS_VISIBLE,0,0,0,0,hwnd,NULL,GetModuleHandleW(NULL),NULL);
            position=CreateWindowExW(0,TRACKBAR_CLASSW,L"",WS_CHILD|TBS_HORZ|TBS_NOTICKS,0,0,0,0,hwnd,NULL,GetModuleHandleW(NULL),NULL);
            SendMessageW(position,TBM_SETRANGE,TRUE,MAKELPARAM(0,1000));
            rp_rebuild_menus(); DragAcceptFiles(hwnd,TRUE); SetTimer(hwnd,1,150,NULL); SetFocus(editor); return 0;
        }
        case WM_SIZE: resize(); return 0;
        case WM_SETFOCUS: SetFocus(editor); return 0;
        case WM_DPICHANGED: {
            dpi=HIWORD(w); RECT *r=(RECT *)l; SetWindowPos(hwnd,NULL,r->left,r->top,r->right-r->left,r->bottom-r->top,SWP_NOZORDER|SWP_NOACTIVATE); currentSize=0; return 0;
        }
        case WM_SYSCOLORCHANGE: apply_colors(); return 0;
        case WM_SETTINGCHANGE: if(currentTheme==0) apply_colors(); return 0;
        case WM_THEMECHANGED: apply_colors(); return 0;
        case WM_ERASEBKGND:
            if(themeBrush) { RECT area; GetClientRect(hwnd,&area); FillRect((HDC)w,&area,themeBrush); return 1; }
            break;
        case WM_CTLCOLORSTATIC:
            if((HWND)l==status&&themeBrush) { SetTextColor((HDC)w,darkTheme?RGB(220,220,220):GetSysColor(COLOR_BTNTEXT)); SetBkColor((HDC)w,darkTheme?RGB(32,32,32):GetSysColor(COLOR_BTNFACE)); return (LRESULT)themeBrush; }
            break;
        case WM_TIMER: rp_tick(); return 0;
        case WM_MOUSEWHEEL: scroll_wheel(w); return 0;
        case WM_HSCROLL: if((HWND)l==position&&LOWORD(w)==TB_ENDTRACK) rp_view(SendMessageW(position,TBM_GETPOS,0,0)/1000.0); return 0;
        case WM_COMMAND:
            if((HWND)l==editor&&HIWORD(w)==EN_CHANGE) { if(!updating) { documentDirty=1; rp_changed(); } }
            // Child notifications carry their HWND in lParam. Only menus and
            // accelerators have lParam == 0; the editor ID overlaps RP_NEW.
            else if(l==0 && HIWORD(w)<=1 && !busy) command(LOWORD(w));
            return 0;
        case WM_DROPFILES: {
            HDROP drop=(HDROP)w; UINT count=DragQueryFileW(drop,0xffffffff,NULL,0);
            for(UINT i=0;i<count;i++) { UINT len=DragQueryFileW(drop,i,NULL,0); wchar_t *path=calloc((size_t)len+1,sizeof(wchar_t)); if(path) { DragQueryFileW(drop,i,path,len+1); char *p=utf8(path); if(p) { rp_open(p); free(p); } free(path); } }
            DragFinish(drop); return 0;
        }
        case WM_CLOSE: rp_action(RP_QUIT); rp_tick(); return 0;
        case WM_QUERYENDSESSION:
            if(!documentDirty&&!busy)return TRUE;
            rp_action(RP_QUIT); rp_tick(); return !IsWindow(hwnd);
        case WM_ENDSESSION: if(w) { DestroyWindow(hwnd); } return 0;
        case WM_DESTROY: KillTimer(hwnd,1); if(themeBrush)DeleteObject(themeBrush); PostQuitMessage(0); return 0;
        default: return DefWindowProcW(hwnd,message,w,l);
    }
}
void rp_run(void) {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    OleInitialize(NULL); HMODULE rich=LoadLibraryW(L"Msftedit.dll"); if(!rich) { MessageBoxW(NULL,L"Cannot load Windows Rich Edit",L"RavnPad",MB_ICONERROR); return; }
    INITCOMMONCONTROLSEX controls={sizeof(controls),ICC_BAR_CLASSES}; InitCommonControlsEx(&controls);
    findMessage=RegisterWindowMessageW(FINDMSGSTRINGW);
    WNDCLASSEXW cls={0}; cls.cbSize=sizeof(cls); cls.lpfnWndProc=procedure; cls.hInstance=GetModuleHandleW(NULL);
    cls.hCursor=LoadCursorW(NULL,IDC_ARROW); cls.hIcon=LoadIconW(cls.hInstance,MAKEINTRESOURCEW(1)); cls.hbrBackground=(HBRUSH)(COLOR_WINDOW+1); cls.lpszClassName=L"RavnPadNative";
    RegisterClassExW(&cls);
    window=CreateWindowExW(0,cls.lpszClassName,L"RavnPad",WS_OVERLAPPEDWINDOW,CW_USEDEFAULT,CW_USEDEFAULT,900,650,NULL,NULL,cls.hInstance,NULL);
    if(!window) { FreeLibrary(rich); OleUninitialize(); return; }
    ACCEL keys[]={{FVIRTKEY|FCONTROL,'N',RP_NEW},{FVIRTKEY|FCONTROL|FSHIFT,'N',RP_NEW_WINDOW},{FVIRTKEY|FCONTROL,'O',RP_OPEN},{FVIRTKEY|FCONTROL,'S',RP_SAVE},{FVIRTKEY|FCONTROL|FSHIFT,'S',RP_SAVE_AS},{FVIRTKEY|FCONTROL,'Q',RP_QUIT},{FVIRTKEY|FCONTROL,'Z',RP_UNDO},{FVIRTKEY|FCONTROL,'Y',RP_REDO},{FVIRTKEY|FCONTROL,'X',RP_CUT},{FVIRTKEY|FCONTROL,'C',RP_COPY},{FVIRTKEY|FCONTROL,'V',RP_PASTE},{FVIRTKEY|FCONTROL,'F',RP_FIND},{FVIRTKEY|FCONTROL,'H',RP_REPLACE},{FVIRTKEY,VK_F3,RP_NEXT},{FVIRTKEY|FCONTROL,'A',RP_SELECT_ALL}};
    accelerators=CreateAcceleratorTableW(keys,sizeof(keys)/sizeof(keys[0]));
    if(smokeTest) {
        const char *sample="Native UTF-8: \xc3\xa6\xc3\xb8\xc3\xa5 \xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e \xf0\x9f\x98\x80\nSecond line";
        rp_document(sample,strlen(sample),0);
        size_t length=0; char *copy=rp_copy_text(&length);
        int valid=copy && length==strlen(sample) && memcmp(copy,sample,length)==0; rp_free_text(copy);
        valid=valid&&((GetWindowLongPtrW(editor,GWL_STYLE)&ES_NOHIDESEL)!=0);
        CHARRANGE end={-1,-1}; SendMessageW(editor,EM_EXSETSEL,0,(LPARAM)&end);
        SendMessageW(editor,EM_REPLACESEL,TRUE,(LPARAM)L"!");
        valid=valid && SendMessageW(editor,EM_CANUNDO,0,0);
        SendMessageW(editor,EM_UNDO,0,0);
        copy=rp_copy_text(&length); valid=valid && copy && strcmp(copy,sample)==0; rp_free_text(copy);
        SendMessageW(editor,EM_REDO,0,0);
        copy=rp_copy_text(&length); valid=valid && copy && length>0 && copy[length-1]=='!'; rp_free_text(copy);
        rp_document("next",4,1); valid=valid && !SendMessageW(editor,EM_CANUNDO,0,0);
        // Exercise a document larger than 200 KB through real Rich Edit scrolling.
        const char *line="Scrolling must preserve this document: abcdefghijklmnopqrstuvwxyz\n";
        size_t lineLength=strlen(line), largeLength=lineLength*5000;
        char *large=malloc(largeLength+1);
        if(!large) valid=0;
        else {
            for(size_t i=0;i<5000;i++)memcpy(large+i*lineLength,line,lineLength);
            large[largeLength]=0;
            rp_document(large,largeLength,0);
            rp_preferences("Consolas",15,0,0);
            ShowWindow(window,SW_SHOWNOACTIVATE); UpdateWindow(window);
            RECT clientRect={0},formatRect={0};
            GetClientRect(editor,&clientRect); SendMessageW(editor,EM_GETRECT,0,(LPARAM)&formatRect);
            valid=valid&&formatRect.top>clientRect.top&&formatRect.left>clientRect.left
                &&formatRect.right<clientRect.right&&formatRect.bottom==clientRect.bottom;
            CHARRANGE mouseSelection={0,215};
            SendMessageW(editor,EM_EXSETSEL,0,(LPARAM)&mouseSelection);
            valid=valid&&editor_command_enabled(RP_COPY)&&editor_command_enabled(RP_CUT)&&editor_command_enabled(RP_SELECT_ALL);
            CHARRANGE caret={5,5}; SendMessageW(editor,EM_EXSETSEL,0,(LPARAM)&caret); selectionAnchor=-1;
            POINTL shiftPoint={0}; SendMessageW(editor,EM_POSFROMCHAR,(WPARAM)&shiftPoint,35);
            SendMessageW(editor,WM_LBUTTONDOWN,MK_SHIFT,MAKELPARAM((short)shiftPoint.x,(short)shiftPoint.y));
            CHARRANGE shifted={0}; SendMessageW(editor,EM_EXGETSEL,0,(LPARAM)&shifted);
            valid=valid&&shifted.cpMin==5&&shifted.cpMax==35;
            SendMessageW(editor,EM_EXSETSEL,0,(LPARAM)&mouseSelection); selectionAnchor=mouseSelection.cpMin;
            SendMessageW(editor,WM_VSCROLL,SB_TOP,0);
            int firstBefore=(int)SendMessageW(editor,EM_GETFIRSTVISIBLELINE,0,0);
            SCROLLINFO scrollBefore={0}; scrollBefore.cbSize=sizeof(scrollBefore); scrollBefore.fMask=SIF_ALL;
            GetScrollInfo(editor,SB_VERT,&scrollBefore);
            UINT smokeWheelLines=3;
            SystemParametersInfoW(SPI_GETWHEELSCROLLLINES,0,&smokeWheelLines,0);
            int wheelScrollingEnabled=smokeWheelLines!=0;
            SendMessageW(editor,WM_MOUSEWHEEL,MAKEWPARAM(0,(WORD)-WHEEL_DELTA),0);
            int firstAfter=(int)SendMessageW(editor,EM_GETFIRSTVISIBLELINE,0,0);
            SCROLLINFO scrollAfter={0}; scrollAfter.cbSize=sizeof(scrollAfter); scrollAfter.fMask=SIF_ALL;
            GetScrollInfo(editor,SB_VERT,&scrollAfter);
            CHARRANGE selectionAfterWheel={0};
            SendMessageW(editor,EM_EXGETSEL,0,(LPARAM)&selectionAfterWheel);
            int interactionValid=mouseSelection.cpMax>mouseSelection.cpMin
                &&(wheelScrollingEnabled?firstAfter>firstBefore:firstAfter==firstBefore)
                &&selectionAfterWheel.cpMin==mouseSelection.cpMin
                &&selectionAfterWheel.cpMax==mouseSelection.cpMax;
            if(!interactionValid) fprintf(stderr,"Mouse/scroll smoke: selection %ld..%ld -> %ld..%ld, first line %d -> %d, scroll %d/%d/%u -> %d/%d/%u\n",
                mouseSelection.cpMin,mouseSelection.cpMax,selectionAfterWheel.cpMin,selectionAfterWheel.cpMax,firstBefore,firstAfter,
                scrollBefore.nPos,scrollBefore.nMax,scrollBefore.nPage,scrollAfter.nPos,scrollAfter.nMax,scrollAfter.nPage);
            valid=valid&&interactionValid;
            SendMessageW(editor,WM_VSCROLL,SB_TOP,0);
            wheelRemainder=0;
            SendMessageW(editor,WM_MOUSEWHEEL,MAKEWPARAM(0,(WORD)-(WHEEL_DELTA/2)),0);
            int firstAfterHalf=(int)SendMessageW(editor,EM_GETFIRSTVISIBLELINE,0,0);
            SendMessageW(editor,WM_MOUSEWHEEL,MAKEWPARAM(0,(WORD)-(WHEEL_DELTA/2)),0);
            int firstAfterFull=(int)SendMessageW(editor,EM_GETFIRSTVISIBLELINE,0,0);
            valid=valid&&firstAfterHalf==0
                &&(wheelScrollingEnabled?firstAfterFull>0:firstAfterFull==0);
            SendMessageW(editor,WM_VSCROLL,SB_PAGEDOWN,0);
            SendMessageW(editor,WM_MOUSEWHEEL,MAKEWPARAM(0,(WORD)-WHEEL_DELTA),0);
            SendMessageW(editor,WM_VSCROLL,SB_BOTTOM,0);
            SendMessageW(editor,WM_VSCROLL,SB_TOP,0);
            // Notifications must never enqueue RP_NEW (editor control ID 1).
            const WORD notifications[]={EN_VSCROLL,EN_HSCROLL,EN_SETFOCUS,EN_KILLFOCUS,EN_UPDATE};
            for(size_t i=0;i<sizeof(notifications)/sizeof(notifications[0]);i++)
                SendMessageW(window,WM_COMMAND,MAKEWPARAM(1,notifications[i]),(LPARAM)editor);
            copy=rp_copy_text(&length);
            valid=valid && copy && length==largeLength && memcmp(copy,large,largeLength)==0;
            rp_free_text(copy); free(large);
        }
        smokeResult=valid?0:1; fprintf(stderr,"Native Win32 smoke test: %s\n",valid?"PASS":"FAIL");
        DestroyWindow(window); if(font)DeleteObject(font); FreeLibrary(rich); OleUninitialize(); return;
    }
    ShowWindow(window,SW_SHOW); UpdateWindow(window); rp_tick();
    MSG msg;
    while(GetMessageW(&msg,NULL,0,0)>0) {
        if(findDialog&&IsDialogMessageW(findDialog,&msg))continue;
        if(TranslateAcceleratorW(window,accelerators,&msg))continue;
        TranslateMessage(&msg); DispatchMessageW(&msg);
    }
    DestroyAcceleratorTable(accelerators); if(font)DeleteObject(font); FreeLibrary(rich); OleUninitialize();
}
void rp_document(const char *value,size_t length,int readonly) {
    updating=1;
    selectionAnchor=-1;
    int count=length<=INT_MAX?MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,value,(int)length,NULL,0):0;
    wchar_t *w=calloc((size_t)count+1,sizeof(wchar_t));
    if(w && count) MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,value,(int)length,w,count); if(w) { SETTEXTEX set={ST_DEFAULT,1200}; SendMessageW(editor,EM_SETTEXTEX,(WPARAM)&set,(LPARAM)w); free(w); }
    apply_colors();
    SendMessageW(editor,EM_EMPTYUNDOBUFFER,0,0); SendMessageW(editor,EM_SETMODIFY,FALSE,0); SendMessageW(editor,EM_SETREADONLY,readonly,0);
    CHARRANGE start={0,0}; SendMessageW(editor,EM_EXSETSEL,0,(LPARAM)&start); SendMessageW(editor,EM_SCROLLCARET,0,0); updating=0;
}
char *rp_copy_text(size_t *length) {
    GETTEXTLENGTHEX size={GTL_NUMCHARS|GTL_PRECISE,1200}; LRESULT count=SendMessageW(editor,EM_GETTEXTLENGTHEX,(WPARAM)&size,0);
    if(count<0) return NULL;
    wchar_t *value=calloc((size_t)count+2,sizeof(wchar_t)); if(!value) return NULL;
    GETTEXTEX get={0}; get.cb=(DWORD)(((size_t)count+2)*sizeof(wchar_t)); get.codepage=1200;
    SendMessageW(editor,EM_GETTEXTEX,(WPARAM)&get,(LPARAM)value);
    // Rich Edit stores paragraph delimiters as CR. Expose LF to the Rust controller.
    for(wchar_t *p=value;*p;p++) if(*p==L'\r') *p=L'\n';
    char *out=utf8(value); free(value); if(out)*length=strlen(out); return out;
}
void rp_free_text(char *text) { free(text); }
void rp_state(const char *title,const char *path,const char *value,int dirty,int working,int readonly,int large) {
    (void)path; (void)large; documentDirty=dirty; text(window,title); text(status,value); busy=working; readonlyDocument=readonly;
    if(findDialog) EnableWindow(findDialog,!working);
    SendMessageW(editor,EM_SETREADONLY,working||readonly,0); EnableWindow(position,!working); ShowWindow(position,readonly?SW_SHOW:SW_HIDE);
    EnableMenuItem(menu,RP_SAVE,MF_BYCOMMAND|((working||readonly)?MF_GRAYED:MF_ENABLED));
    EnableMenuItem(menu,RP_SAVE_AS,MF_BYCOMMAND|((working||readonly)?MF_GRAYED:MF_ENABLED));
}
void rp_preferences(const char *name,double points,int spell,int language) {
    wchar_t *family=wide(name); if(!family)return;
    if(wcscmp(family,currentFont)||currentSize!=points) {
        wcsncpy_s(currentFont,LF_FACESIZE,family,_TRUNCATE); currentSize=points;
        HFONT next=CreateFontW(-MulDiv((int)(points*10),dpi,720),0,0,0,FW_NORMAL,FALSE,FALSE,FALSE,DEFAULT_CHARSET,OUT_DEFAULT_PRECIS,CLIP_DEFAULT_PRECIS,CLEARTYPE_QUALITY,DEFAULT_PITCH,family[0]?family:L"Consolas");
        if(next) { SendMessageW(editor,WM_SETFONT,(WPARAM)next,TRUE); if(font)DeleteObject(font); font=next; }
    }
    free(family);
    if(currentSpell!=spell) { currentSpell=spell; SendMessageW(editor,EM_SETEDITSTYLE,spell?SES_CTFALLOWPROOFING:0,SES_CTFALLOWPROOFING); }
    CheckMenuRadioItem(menu,100,114,100+language,MF_BYCOMMAND);
    CheckMenuItem(menu,RP_SPELL,MF_BYCOMMAND|(spell?MF_CHECKED:MF_UNCHECKED));
}
void rp_find_result(size_t length) {
    if(!length) { wchar_t *message=wide(rp_label(RP_NO_MATCHES)); MessageBoxW(window,message?message:L"No matches found",L"RavnPad",MB_OK|MB_ICONINFORMATION); free(message); return; }
    CHARRANGE match={0,(LONG)length}; SendMessageW(editor,EM_EXSETSEL,0,(LPARAM)&match); SendMessageW(editor,EM_SCROLLCARET,0,0);
}
void rp_wrap(int enabled) {
    if(currentWrap==enabled) return;
    currentWrap=enabled;
    SendMessageW(editor,EM_SETTARGETDEVICE,0,enabled?0:1);
    ShowScrollBar(editor,SB_HORZ,enabled?FALSE:TRUE);
    CheckMenuItem(menu,RP_WRAP,MF_BYCOMMAND|(enabled?MF_CHECKED:MF_UNCHECKED));
    InvalidateRect(editor,NULL,TRUE);
}
void rp_theme(int preference) {
    if(currentTheme==preference) return;
    currentTheme=preference;
    apply_colors();
    if(themeMenu) CheckMenuRadioItem(themeMenu,RP_THEME_SYSTEM,RP_THEME_DARK,RP_THEME_SYSTEM+preference,MF_BYCOMMAND);
}
double rp_read_position(void) {
    SCROLLINFO info={0}; info.cbSize=sizeof(info); info.fMask=SIF_RANGE|SIF_PAGE|SIF_POS;
    if(!GetScrollInfo(editor,SB_VERT,&info)) return 0;
    int maximum=info.nMax-(int)(info.nPage?info.nPage-1:0);
    return maximum>info.nMin?(double)(info.nPos-info.nMin)/(double)(maximum-info.nMin):0;
}
void rp_restore_position(double fraction) {
    SCROLLINFO info={0}; info.cbSize=sizeof(info); info.fMask=SIF_RANGE|SIF_PAGE;
    if(!GetScrollInfo(editor,SB_VERT,&info)) return;
    int maximum=info.nMax-(int)(info.nPage?info.nPage-1:0);
    info.fMask=SIF_POS; info.nPos=info.nMin+(int)((maximum-info.nMin)*(fraction<0?0:fraction>1?1:fraction));
    SetScrollInfo(editor,SB_VERT,&info,TRUE); SendMessageW(editor,WM_VSCROLL,MAKEWPARAM(SB_THUMBPOSITION,info.nPos),0);
}
void rp_close(void) { DestroyWindow(window); }

int rp_smoke_test(void) { smokeTest=1; smokeResult=1; rp_run(); return smokeResult; }

void rp_lock(void) { busy=1; SendMessageW(editor,EM_SETREADONLY,TRUE,0); }

void rp_cancel_close(void) {}

int rp_confirm(const char *title,const char *body,const char *accept,const char *discard,const char *cancel) {
    wchar_t *wideTitle=wide(title), *wideBody=wide(body), *wideAccept=wide(accept), *wideDiscard=wide(discard), *wideCancel=wide(cancel);
    enum { CONFIRM_ACCEPT=1001, CONFIRM_DISCARD=1002 };
    TASKDIALOG_BUTTON buttons[]={{CONFIRM_ACCEPT,wideAccept},{CONFIRM_DISCARD,wideDiscard},{IDCANCEL,wideCancel}};
    TASKDIALOGCONFIG config={0}; config.cbSize=sizeof(config); config.hwndParent=window;
    config.dwFlags=TDF_POSITION_RELATIVE_TO_WINDOW|TDF_SIZE_TO_CONTENT;
    config.pszWindowTitle=L"RavnPad"; config.pszMainInstruction=wideTitle; config.pszContent=wideBody;
    config.pszMainIcon=TD_WARNING_ICON; config.cButtons=3; config.pButtons=buttons; config.nDefaultButton=CONFIRM_ACCEPT;
    int selected=IDCANCEL;
    HRESULT result=TaskDialogIndirect(&config,&selected,NULL,NULL);
    free(wideTitle); free(wideBody); free(wideAccept); free(wideDiscard); free(wideCancel);
    if(FAILED(result)) return 0;
    return selected==CONFIRM_ACCEPT?1:selected==CONFIRM_DISCARD?2:0;
}
