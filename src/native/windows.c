#define _WIN32_WINNT 0x0A00
#define _RICHEDIT_VER 0x0800
#include <windows.h>
#include <commctrl.h>
#include <commdlg.h>
#include <richedit.h>
#include <shellapi.h>
#include <ole2.h>
#include <aclapi.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>
#include <limits.h>
#include <stdio.h>
#include "bridge.h"

static HWND window, editor, status, position, findDialog;
static HMENU menu;
static HFONT font;
static HACCEL accelerators;
static UINT findMessage;
static FINDREPLACEW find;
static wchar_t query[1024], replacement[1024], currentFont[LF_FACESIZE];
static double currentSize;
static int updating, busy, readonlyDocument, currentSpell, smokeTest, smokeResult, documentDirty;
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
void rp_rebuild_menus(void) {
    HMENU previous=menu; menu=CreateMenu();
    HMENU file=submenu(menu,RP_FILE);
    entry(file,RP_NEW,L"\tCtrl+N"); entry(file,RP_OPEN,L"\tCtrl+O");
    entry(file,RP_SAVE,L"\tCtrl+S"); entry(file,RP_SAVE_AS,L"\tCtrl+Shift+S");
    AppendMenuW(file,MF_SEPARATOR,0,NULL); entry(file,RP_RECOVERY,NULL); entry(file,RP_QUIT,L"\tAlt+F4");
    HMENU edit=submenu(menu,RP_EDIT);
    entry(edit,RP_UNDO,L"\tCtrl+Z"); entry(edit,RP_REDO,L"\tCtrl+Y"); AppendMenuW(edit,MF_SEPARATOR,0,NULL);
    entry(edit,RP_CUT,L"\tCtrl+X"); entry(edit,RP_COPY,L"\tCtrl+C"); entry(edit,RP_PASTE,L"\tCtrl+V"); entry(edit,RP_SELECT_ALL,L"\tCtrl+A");
    AppendMenuW(edit,MF_SEPARATOR,0,NULL); entry(edit,RP_FIND,L"\tCtrl+F"); entry(edit,RP_REPLACE,L"\tCtrl+H"); entry(edit,RP_NEXT,L"\tF3");
    HMENU settings=submenu(menu,RP_SETTINGS); entry(settings,RP_FONT,NULL); entry(settings,RP_SPELL,NULL);
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
    int margin=MulDiv(12,dpi,96); InflateRect(&textRect,-margin,-margin);
    SendMessageW(editor,EM_SETRECT,0,(LPARAM)&textRect);
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
    find.Flags=FR_DOWN; find.lpstrFindWhat=query; find.wFindWhatLen=1024;
    find.lpstrReplaceWith=replacement; find.wReplaceWithLen=1024;
    findDialog=replace?ReplaceTextW(&find):FindTextW(&find);
}
static int find_next(int wrap) {
    if(!query[0]) { show_find(0); return 0; }
    CHARRANGE selection; SendMessageW(editor,EM_EXGETSEL,0,(LPARAM)&selection);
    FINDTEXTEXW search={0}; search.lpstrText=query;
    int down=(find.Flags&FR_DOWN)!=0;
    search.chrg.cpMin=down?selection.cpMax:selection.cpMin;
    search.chrg.cpMax=down?-1:0;
    WPARAM flags=find.Flags&(FR_DOWN|FR_MATCHCASE|FR_WHOLEWORD);
    LRESULT result=SendMessageW(editor,EM_FINDTEXTEXW,flags,(LPARAM)&search);
    if(result==-1 && wrap) {
        search.chrg.cpMin=down?0:GetWindowTextLengthW(editor); search.chrg.cpMax=down?-1:0;
        result=SendMessageW(editor,EM_FINDTEXTEXW,flags,(LPARAM)&search);
    }
    if(result==-1) return 0;
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
        case RP_NEXT: find_next(1); return;
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
        if(event->Flags&FR_DIALOGTERM) findDialog=NULL;
        else if(event->Flags&FR_FINDNEXT) find_next(1);
        else if(event->Flags&FR_REPLACE) { replace_selection(); find_next(1); }
        else if((event->Flags&FR_REPLACEALL)&&!readonlyDocument&&!busy&&query[0]) {
            CHARRANGE start={0,0}; SendMessageW(editor,EM_EXSETSEL,0,(LPARAM)&start); find.Flags|=FR_DOWN;
            SendMessageW(editor,WM_SETREDRAW,FALSE,0);
            while(find_next(0)) replace_selection();
            SendMessageW(editor,WM_SETREDRAW,TRUE,0); InvalidateRect(editor,NULL,TRUE);
        }
        return 0;
    }
    switch(message) {
        case WM_CREATE: {
            window=hwnd; dpi=GetDpiForWindow(hwnd);
            editor=CreateWindowExW(0,MSFTEDIT_CLASS,L"",WS_CHILD|WS_VISIBLE|WS_VSCROLL|ES_MULTILINE|ES_AUTOVSCROLL|ES_WANTRETURN,0,0,0,0,hwnd,(HMENU)1,GetModuleHandleW(NULL),NULL);
            if(!editor) return -1;
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
        case WM_SYSCOLORCHANGE: SendMessageW(editor,EM_SETBKGNDCOLOR,0,GetSysColor(COLOR_WINDOW)); InvalidateRect(editor,NULL,TRUE); return 0;
        case WM_TIMER: rp_tick(); return 0;
        case WM_HSCROLL: if((HWND)l==position&&LOWORD(w)==TB_ENDTRACK) rp_view(SendMessageW(position,TBM_GETPOS,0,0)/1000.0); return 0;
        case WM_COMMAND:
            if((HWND)l==editor&&HIWORD(w)==EN_CHANGE) { if(!updating) { documentDirty=1; rp_changed(); } }
            else if(!busy) command(LOWORD(w)); return 0;
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
        case WM_DESTROY: KillTimer(hwnd,1); PostQuitMessage(0); return 0;
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
    ACCEL keys[]={{FVIRTKEY|FCONTROL,'N',RP_NEW},{FVIRTKEY|FCONTROL,'O',RP_OPEN},{FVIRTKEY|FCONTROL,'S',RP_SAVE},{FVIRTKEY|FCONTROL|FSHIFT,'S',RP_SAVE_AS},{FVIRTKEY|FCONTROL,'Q',RP_QUIT},{FVIRTKEY|FCONTROL,'F',RP_FIND},{FVIRTKEY|FCONTROL,'H',RP_REPLACE},{FVIRTKEY,VK_F3,RP_NEXT},{FVIRTKEY|FCONTROL,'A',RP_SELECT_ALL}};
    accelerators=CreateAcceleratorTableW(keys,sizeof(keys)/sizeof(keys[0]));
    if(smokeTest) {
        const char *sample="Native UTF-8: \xc3\xa6\xc3\xb8\xc3\xa5 \xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e \xf0\x9f\x98\x80\nSecond line";
        rp_document(sample,strlen(sample),0);
        size_t length=0; char *copy=rp_copy_text(&length);
        int valid=copy && length==strlen(sample) && memcmp(copy,sample,length)==0; rp_free_text(copy);
        CHARRANGE end={-1,-1}; SendMessageW(editor,EM_EXSETSEL,0,(LPARAM)&end);
        SendMessageW(editor,EM_REPLACESEL,TRUE,(LPARAM)L"!");
        valid=valid && SendMessageW(editor,EM_CANUNDO,0,0);
        SendMessageW(editor,EM_UNDO,0,0);
        copy=rp_copy_text(&length); valid=valid && copy && strcmp(copy,sample)==0; rp_free_text(copy);
        SendMessageW(editor,EM_REDO,0,0);
        copy=rp_copy_text(&length); valid=valid && copy && length>0 && copy[length-1]=='!'; rp_free_text(copy);
        rp_document("next",4,1); valid=valid && !SendMessageW(editor,EM_CANUNDO,0,0);
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
    int count=length<=INT_MAX?MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,value,(int)length,NULL,0):0;
    wchar_t *w=calloc((size_t)count+1,sizeof(wchar_t));
    if(w && count) MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,value,(int)length,w,count); if(w) { SETTEXTEX set={ST_DEFAULT,1200}; SendMessageW(editor,EM_SETTEXTEX,(WPARAM)&set,(LPARAM)w); free(w); }
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
void rp_state(const char *title,const char *path,const char *value,int dirty,int working,int readonly) {
    (void)path; documentDirty=dirty; text(window,title); text(status,value); busy=working; readonlyDocument=readonly;
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
void rp_close(void) { DestroyWindow(window); }

int rp_smoke_test(void) { smokeTest=1; smokeResult=1; rp_run(); return smokeResult; }

void rp_lock(void) { busy=1; SendMessageW(editor,EM_SETREADONLY,TRUE,0); }

void rp_cancel_close(void) {}

// Copy owner, primary group and DACL through handles; never enable privileges
// or continue with inherited/broader permissions if preserving them fails.
DWORD rp_copy_security(const wchar_t *source,const wchar_t *target) {
    DWORD sharing=FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE;
    HANDLE input=CreateFileW(source,READ_CONTROL,sharing,NULL,OPEN_EXISTING,0,NULL);
    if(input==INVALID_HANDLE_VALUE)return GetLastError();
    HANDLE output=CreateFileW(target,WRITE_DAC|WRITE_OWNER,sharing,NULL,OPEN_EXISTING,0,NULL);
    if(output==INVALID_HANDLE_VALUE) { DWORD error=GetLastError(); CloseHandle(input); return error; }
    PSID owner=NULL,group=NULL; PACL dacl=NULL; PSECURITY_DESCRIPTOR descriptor=NULL;
    SECURITY_INFORMATION info=OWNER_SECURITY_INFORMATION|GROUP_SECURITY_INFORMATION|DACL_SECURITY_INFORMATION;
    DWORD error=GetSecurityInfo(input,SE_FILE_OBJECT,info,&owner,&group,&dacl,NULL,&descriptor);
    if(error==ERROR_SUCCESS) {
        SECURITY_DESCRIPTOR_CONTROL control=0; DWORD revision=0;
        if(!GetSecurityDescriptorControl(descriptor,&control,&revision))error=GetLastError();
        else {
            info|=(control&SE_DACL_PROTECTED)?PROTECTED_DACL_SECURITY_INFORMATION:UNPROTECTED_DACL_SECURITY_INFORMATION;
            error=SetSecurityInfo(output,SE_FILE_OBJECT,info,owner,group,dacl,NULL);
        }
    }
    if(descriptor)LocalFree(descriptor);
    CloseHandle(output); CloseHandle(input); return error;
}
