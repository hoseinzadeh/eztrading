#pragma once

#include <cstring>
#include <cassert>
#include <mutex>
#include <utils.h>
#include <sig.h>
#include <ticker.h>
#include <scheduler.h>
#include <account.h>
#include <contents.h>
#include <sstream>

extern const char *format(const char *fmt, ...);

namespace sml {
    class machine {
    public:
        virtual void die() {}
    };
}
