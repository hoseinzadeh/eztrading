#pragma once
#include <cstdlib>
#include <iostream>
#include <thread>
#include <unistd.h>
#include <cmath>
#include <vector>
#include <cstdint>
#include <cassert>
#include <cstring>
#include <chrono>

typedef int64_t int64;
typedef uint64_t uint64;
typedef int32_t int32;
typedef uint32_t uint32;
typedef int16_t int16;
typedef uint16_t uint16;
typedef int8_t int8;
typedef uint8_t uint8;

using std::chrono::system_clock;

constexpr __uint64_t to_int(const char* x) {
    __uint64_t y = 0;
    for (int i = 0; x[i] != '\0'; ++i)
    {
        y *= 10;
        if ('0' <= x[i] && x[i] <= '9')
            y += x[i] - '0';
    }
    return y;
}

constexpr system_clock::duration operator""_ms(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::milliseconds(val);
}

constexpr system_clock::duration operator""_millisec(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::milliseconds(val);
}

constexpr system_clock::duration operator""_millisecs(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val != 1);
    return std::chrono::milliseconds(val);
}

constexpr system_clock::duration operator""_millisecond(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::milliseconds(val);
}

constexpr system_clock::duration operator""_milliseconds(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val != 1);
    return std::chrono::milliseconds(val);
}

constexpr system_clock::duration operator""_sec(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::seconds(val);
}

constexpr system_clock::duration operator""_secs(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val != 1);
    return std::chrono::seconds(val);
}

constexpr system_clock::duration operator""_second(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::seconds(val);
}

constexpr system_clock::duration operator""_seconds(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val != 1);
    return std::chrono::seconds(val);
}

constexpr system_clock::duration operator""_min(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::minutes(val);
}

constexpr system_clock::duration operator""_mins(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val != 1);
    return std::chrono::minutes(val);
}

constexpr system_clock::duration operator""_minute(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::minutes(val);
}

constexpr system_clock::duration operator""_minutes(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val != 1);
    return std::chrono::minutes(val);
}

constexpr system_clock::duration operator""_hr(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::hours(val);
}

constexpr system_clock::duration operator""_hrs(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val != 1);
    return std::chrono::hours(val);
}

constexpr system_clock::duration operator""_hour(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::hours(val);
}

constexpr system_clock::duration operator""_hours(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val != 1);
    return std::chrono::hours(val);
}

constexpr system_clock::duration operator""_day(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::days(val);
}

constexpr system_clock::duration operator""_days(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val != 1);
    return std::chrono::days(val);
}

constexpr system_clock::duration operator""_week(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::weeks(val);
}

constexpr system_clock::duration operator""_weeks(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val != 1);
    return std::chrono::weeks(val);
}

constexpr system_clock::duration operator""_month(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::months(val);
}

constexpr system_clock::duration operator""_months(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val != 1);
    return std::chrono::months(val);
}

constexpr system_clock::duration operator""_year(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val == 1);
    return std::chrono::years(val);
}

constexpr system_clock::duration operator""_years(const char* x)
{
    __uint64_t val = to_int(x);
    assert(val != 1);
    return std::chrono::years(val);
}

class duration: public system_clock::duration {
public:
    duration(const system_clock::duration &other = system_clock::duration()) : system_clock::duration(other) {}
    static system_clock::duration years(int i) { return std::chrono::years(i); }
    static system_clock::duration months(int i) { return std::chrono::months(i); }
    static system_clock::duration weeks(int i) { return std::chrono::weeks(i); }
    static system_clock::duration days(int i) { return std::chrono::days(i); }
    static system_clock::duration hours(int i) { return std::chrono::hours(i); }
    static system_clock::duration minutes(int i) { return std::chrono::minutes(i); }
    static system_clock::duration seconds(int i) { return std::chrono::seconds(i); }
    static system_clock::duration milliseconds(int i) { return std::chrono::milliseconds(i); }
    static system_clock::time_point days_at(int d, int h, int m, int s) {
        std::time_t now(std::time(0));
        std::tm *tm = localtime(&now);
        tm->tm_hour += 24 * d;
        std::time_t t = std::mktime(tm); 
        tm = localtime(&t);
        tm->tm_isdst = 0;
        tm->tm_hour = h;
        tm->tm_min = m;
        tm->tm_sec = s;
        return system_clock::from_time_t(std::mktime(tm));
    }
    static system_clock::time_point months_at(int m, int d, int h, int M, int s) {
        std::time_t now(std::time(0));
        std::tm *tm = localtime(&now);
        tm->tm_mday = d;
        tm->tm_mon += m;
        std::time_t t = std::mktime(tm); 
        tm = localtime(&t);
        tm->tm_isdst = 0;
        tm->tm_hour = h;
        tm->tm_min = M;
        tm->tm_sec = s;
        return system_clock::from_time_t(std::mktime(tm));
    }
    static system_clock::time_point years_at(int y, int m, int d, int h, int M, int s) {
        std::time_t now(std::time(0));
        std::tm *tm = localtime(&now);
        tm->tm_mday = d;
        tm->tm_mon = m;
        tm->tm_year += y;
        std::time_t t = std::mktime(tm); 
        tm = localtime(&t);
        tm->tm_isdst = 0;
        tm->tm_hour = h;
        tm->tm_min = M;
        tm->tm_sec = s;
        return system_clock::from_time_t(std::mktime(tm));
    }
};
