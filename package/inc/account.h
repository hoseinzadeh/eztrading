#pragma once

#include <unordered_map>
#include <ticker.h>

class Position {
public:
    Ticker *ticker;
    float shares;

    Position(Ticker *ticker = NULL): ticker(ticker), shares(0) {}

    float value() {
        if (ticker) return ticker->close * shares;
        return 0;
    }

    operator const float() const { return shares; }
    template <class T> bool  operator<(const T &val) { return shares < val; }
    template <class T> bool  operator>(const T &val) { return shares > val; }
    template <class T> bool operator<=(const T &val) { return shares <= val; }
    template <class T> bool operator>=(const T &val) { return shares >= val; }
    template <class T> bool operator==(const T &val) { return shares == val; }
    template <class T> bool operator!=(const T &val) { return shares != val; }
};

class Account {
private:
    std::unordered_map<std::string, Position> positions;
public:
    float cash;
    // TODO: Change there to place order
    //  signal order;
    //  listen order;
    //  state PLACE: goto WAIT on order.is_placed() {
    //      order = account.request(Buy(sym, shares));
    //  }
    //  state WAIT: goto DONE on order.is_executed();
    Position &buy(const Symbol &sym, float shares, Worker *job);
    Position &buy(const Symbol &sym, float shares, float &purchased, Worker *job);
    Position &sell(const Symbol &sym, float shares, Worker *job);
    Position &position(const Symbol &sym);
};

extern Account SandBox;

