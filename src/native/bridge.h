#ifndef RAVNPAD_BRIDGE_H
#define RAVNPAD_BRIDGE_H
#include <stddef.h>
// All callbacks run on the UI thread and enqueue work; never retain string pointers.
enum { RP_NEW=1, RP_OPEN, RP_SAVE, RP_SAVE_AS, RP_QUIT, RP_UPDATE, RP_SPELL,
       RP_RECOVERY, RP_FONT, RP_FIND, RP_REPLACE, RP_ABOUT,
       RP_FILE=20, RP_SETTINGS, RP_LANGUAGE, RP_HELP, RP_EDIT, RP_UNDO,
       RP_REDO, RP_CUT, RP_COPY, RP_PASTE, RP_SELECT_ALL, RP_NEXT, RP_PREVIOUS,
       RP_REPLACE_ALL, RP_CANCEL, RP_RECENT, RP_CLOSE, RP_WINDOW, RP_MINIMIZE, RP_ZOOM, RP_HIDE, RP_HIDE_OTHERS, RP_SHOW_ALL, RP_SERVICES,
       RP_NEW_WINDOW=44, RP_WRAP=45, RP_THEME=46, RP_THEME_SYSTEM=47,
       RP_THEME_LIGHT=48, RP_THEME_DARK=49, RP_NO_MATCHES=50, RP_WHOLE_WORD=51,
       RP_AGENT=52, RP_ENABLE_AGENT=53, RP_AGENT_ENABLED=54,
       RP_DISABLE_AGENT=55, RP_AGENT_HELP=56 };
void rp_tick(void);
void rp_action(int command);
void rp_changed(void);
void rp_open(const char *path);
void rp_font(const char *name, double points);
void rp_view(double fraction);
int rp_find_large(const char *query, int backwards, int match_case, int whole_word);
const char *rp_label(int command);
void rp_run(void);
int rp_smoke_test(void);
void rp_document(const char *text, size_t length, int readonly);
// Replace the editable buffer as one undoable action without clearing history.
void rp_replace_text(const char *text, size_t length);
// Returned buffer is malloc-owned by the native side; release with rp_free_text.
char *rp_copy_text(size_t *length);
void rp_free_text(char *text);
void rp_state(const char *title, const char *path, const char *status, int dirty, int busy, int readonly, int large);
void rp_find_result(size_t length);
void rp_preferences(const char *font, double points, int spell, int language);
void rp_wrap(int enabled);
void rp_theme(int preference);
void rp_agent(int enabled);
double rp_read_position(void);
void rp_restore_position(double fraction);
void rp_rebuild_menus(void);
void rp_close(void);
void rp_lock(void);
void rp_cancel_close(void);
int rp_confirm(const char *title, const char *body, const char *accept, const char *discard, const char *cancel);
#endif
