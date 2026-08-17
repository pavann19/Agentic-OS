#include "desktop.h"
#include "graphics.h"

// Beautiful modern color palette
#define COLOR_BACKGROUND  0x001E1E2E // Catppuccin Mocha Base
#define COLOR_TASKBAR     0x0011111B // Darker base
#define COLOR_WINDOW_BG   0x00313244 // Surface
#define COLOR_TITLEBAR    0x0089B4FA // Blue
#define COLOR_ACCENT      0x00F38BA8 // Red (Close Button)

void draw_desktop(void) {
    uint32_t width = get_screen_width();
    uint32_t height = get_screen_height();
    uint32_t taskbar_height = 40;

    // 1. Draw the background
    draw_rect(0, 0, width, height, COLOR_BACKGROUND);

    // 2. Draw the Taskbar
    draw_rect(0, height - taskbar_height, width, taskbar_height, COLOR_TASKBAR);

    // 3. Draw a "Welcome" Window in the center
    uint32_t win_width = 600;
    uint32_t win_height = 400;
    uint32_t win_x = (width - win_width) / 2;
    uint32_t win_y = (height - win_height) / 2;

    // Window shadow (simple drop shadow)
    draw_rect(win_x + 10, win_y + 10, win_width, win_height, 0x00000000); // Black shadow

    // Window body
    draw_rect(win_x, win_y, win_width, win_height, COLOR_WINDOW_BG);

    // Window Titlebar
    uint32_t titlebar_height = 30;
    draw_rect(win_x, win_y, win_width, titlebar_height, COLOR_TITLEBAR);

    // Close button
    draw_rect(win_x + win_width - 30, win_y + 5, 20, 20, COLOR_ACCENT);
}
