#pragma once
#include <stdint.h>

void serial_init(void);
int serial_is_ready(void);
void serial_write_char(char c);
void serial_write_string(const char* str);
