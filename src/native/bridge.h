#ifndef RAVNPAD_BRIDGE_H
#define RAVNPAD_BRIDGE_H
#include <stddef.h>
// All callbacks run on the UI thread and enqueue work; never retain string pointers.
enum { RP_NEW=1, RP_OPEN, RP_SAVE, RP_SAVE_AS, RP_QUIT, RP_UPDATE, RP_SPELL,
       RP_RECOVERY, RP_FONT, RP_FIND, RP_REPLACE, RP_ABOUT,
       RP_FILE=20, RP_SETTINGS, RP_LANGUAGE, RP_HELP, RP_EDIT, RP_UNDO,
       RP_REDO, RP_CUT, RP_COPY, RP_PASTE, RP_SELECT_ALL, RP_NEXT, RP_PREVIOUS,
       RP_REPLACE_ALL, RP_CANCEL, RP_RECENT, RP_CLOSE, RP_WINDOW, RP_MINIMIZE, RP_ZOOM, RP_HIDE, RP_HIDE_OTHERS, RP_SHOW_ALL, RP_SERVICES };
void rp_tick(void);
void rp_action(int command);
void rp_changed(void);
void rp_open(const char *path);
void rp_font(const char *name, double points);
void rp_view(double fraction);
const char *rp_label(int command);
void rp_run(void);
int rp_smoke_test(void);
void rp_document(const char *text, size_t length, int readonly);
// Returned buffer is malloc-owned by the native side; release with rp_free_text.
char *rp_copy_text(size_t *length);
void rp_free_text(char *text);
void rp_state(const char *title, const char *path, const char *status, int dirty, int busy, int readonly);
void rp_preferences(const char *font, double points, int spell, int language);
void rp_rebuild_menus(void);
void rp_close(void);
void rp_lock(void);
void rp_cancel_close(void);
#endif
