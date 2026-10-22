#pragma once

#include <sig.h>
#include <unordered_map>

/***** Temporary *****/

typedef std::string Symbol;

class Ticker {
public:
    int64_t timestamp;
    float open, high, low, close;
    int volume;

    operator const float() const { return close; }
    bool operator<(const float &val) { return close < val; }
    bool operator>(const float &val) { return close > val; }
    bool operator<=(const float &val) { return close <= val; }
    bool operator>=(const float &val) { return close >= val; }
    bool operator==(const float &val) { return close == val; }
    bool operator!=(const float &val) { return close != val; }
};

extern std::unordered_map<Symbol, Signal<Ticker>> Tickers;