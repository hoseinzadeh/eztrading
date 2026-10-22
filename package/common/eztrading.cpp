#include <eztrading.h>


const char *format(const char *fmt, ...) {
    static thread_local char buf[1024];
    va_list args;
    va_start(args, fmt);
    snprintf(buf, 1024, fmt, args);
    va_end(args);
    return buf;
}