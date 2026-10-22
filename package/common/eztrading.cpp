#include <eztrading.h>


const char *format(const char *fmt, ...) {
    static thread_local char buf[1024];
    va_list args;
    va_start(args, fmt);
    snprintf(buf, 1024, fmt, args);
    va_end(args);
    return buf;
}

/// Called by the job server (through scheduler.so) when a new price arrives. Setting the value and
/// waking the listeners are separate calls because each scheduler wakes only its own jobs.
extern "C" void ezt_set_ticker(const char *symbol, int64_t timestamp, float open, float high,
                              float low, float close, int volume) {
    static std::mutex m;
    std::lock_guard<std::mutex> lock(m);
    Signal<Ticker> &sig = Tickers[symbol];
    Ticker &t = sig.mut();
    t.timestamp = timestamp; t.open = open; t.high = high; t.low = low; t.close = close; t.volume = volume;
}

extern "C" void ezt_emit_ticker(const char *symbol, Scheduler *scheduler) {
    static std::mutex m;
    std::lock_guard<std::mutex> lock(m);
    auto it = Tickers.find(symbol);
    if (it != Tickers.end()) it->second.emit_to(scheduler);
}
