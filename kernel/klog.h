#pragma once
#include <stdint.h>

void klog_init(void);
void klog_info(const char* format, ...);
void klog_warn(const char* format, ...);
void klog_error(const char* format, ...);
void panic(const char* reason);

#define ASSERT(expr) do { if (!(expr)) panic("assertion failed: " #expr); } while (0)
