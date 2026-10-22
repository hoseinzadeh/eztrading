#include <account.h>

std::unordered_map<Symbol, Signal<Ticker>> Tickers;
Account SandBox;

Position &Account::buy(const Symbol &sym, float shares, Worker *job) {
    Position &pos = positions[sym];
    if (job->is_preprocessing) return pos;
    pos.shares += shares;
    // cash -= shares * ticker->close;
    return pos;
}

Position &Account::buy(const Symbol &sym, float shares, float &purchased, Worker *job) {
    Position &pos = positions[sym];
    if (job->is_preprocessing) return pos;
    pos.shares += purchased = shares;
    // cash -= shares * ticker->close;
    return pos;
}

Position &Account::sell(const Symbol &sym, float shares, Worker *job) {
    Position &pos = positions[sym];
    if (job->is_preprocessing) return pos;
    float val = std::min(pos.shares, shares);
    // cash += ticker->close * val;
    pos.shares -= val;
    return pos;
}

Position &Account::position(const Symbol &sym) {
    return positions[sym];
}