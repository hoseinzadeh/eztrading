
#pragma once

#include <atomic>
#include <mutex>
#include <shared_mutex>
#include <scheduler.h>
#include <unordered_set>
#include <filesystem>
#include <cstdarg>
#include <sys/types.h>
#include <sys/stat.h>
#include <sys/mman.h>
#include <unistd.h>
#include <fcntl.h>
#include <fstream>
#include <vec.h>

#define MAX_HISTORY_SIZE (16 << 10)

class SignalCore {
protected:
    mutable std::shared_mutex mutex_;
    mutable std::unordered_set<Worker*> attached_jobs; // The attached_jobs that this signal should waken when changes
    std::atomic<std::chrono::system_clock::time_point> __update_time;

    inline system_clock::time_point update() {
        auto now = system_clock::now();
        __update_time.store(now);
        return now;
    }

public:
    void add_listener(Worker *job) const {
        std::unique_lock lock(mutex_);
        if (!attached_jobs.contains(job))
            attached_jobs.insert(job);
    }

    inline void emit_later(Worker *job) const {
        job->emit_later(const_cast<SignalCore*>(this));
    }

    inline void emit(Scheduler *scheduler) const {
        std::shared_lock lock(mutex_);
        for (Worker *j: attached_jobs) scheduler->submit(j);
    }

    /// Wake only the attached jobs that run on `scheduler`
    inline void emit_to(Scheduler *scheduler) const {
        std::shared_lock lock(mutex_);
        for (Worker *j: attached_jobs) if (j->owned_by(scheduler)) scheduler->submit(j);
    }

    // Check if the __update_time date is greater or equal to the job's previous cycle
    bool changed(Worker *job) const {
        // If the change was during the previous cycle, it can be caught
        return __update_time.load() >= job->__prev_cycle;
    }
};

template <class T>
class Signal: public SignalCore {
    struct Data {
        T data;
        system_clock::time_point timestamp;

        bool operator<(const Data &other) const {
            return timestamp < other.timestamp;
        }
        
        bool operator<(const system_clock::time_point &t) const {
            return timestamp < t;
        }
        
        bool operator==(const Data &other) const
        {
            return data == other.data && timestamp == other.timestamp;
        }
    };
    mutable __shared_vector<Data> data;
    mutable std::atomic_bool has_history;
    mutable std::shared_mutex hist_mutex;

    T &mut_ref() {
        std::unique_lock lock(hist_mutex);
        auto timestamp = update();
        Data cur = data.back();
        cur.timestamp = timestamp;
        data.push_back(cur);
        return data.back().data;
    }

    static void *map_file(const char *path, size_t size, bool &reset) {
        int fdout;
        void *dst;

        {
            std::ifstream f(path);
            reset = !f.good();
        }

        /* open/create the output file */
        if ((fdout = open (path, O_RDWR | O_CREAT, (mode_t)0600 )) < 0)//edited here
        {
            printf ("can't create %s for writing", path);
            return 0;
        }

        /* go to the location corresponding to the last byte */
        if (lseek (fdout, size, SEEK_SET) == -1)
        {
            printf ("lseek error");
            return 0;
        }

        /* write a dummy byte at the last location */
        if (write (fdout, "", 1) != 1)
        {
            printf ("write error");
            return 0;
        }

        /* mmap the output file */
        if ((dst = mmap (0, size, PROT_READ | PROT_WRITE, MAP_SHARED, fdout, 0)) == (caddr_t) -1)
        {
            printf ("mmap error for output");
            return 0;
        }

        return dst;
    }

public:
    Signal(const T &val=T()) : has_history(false) {
        data.push_back(Data { val, system_clock::now() });
    }
    
    Signal(char *mmap) : has_history(true) {
        data.reconstruct_from_file(mmap);
    }

    bool attach_history(Worker *job, const char *path) const {
        bool had_history = false;
        if (has_history.compare_exchange_strong(had_history, true)) {
            static std::mutex filenames_mutex;
            static std::unordered_set<std::string> filenames;
            const std::lock_guard<std::mutex> lock(filenames_mutex);

            const char *env_p = std::getenv("USERS_DATA");
            char full_path[128];
            
            if (env_p) {
                snprintf(full_path, 128, "%s/%u", env_p, job->__run_id);
                mkdir(full_path, 0777);
                snprintf(full_path, 128, "%s/%u/%s", env_p, job->__run_id, path);
            } else {
                mkdir("/Users/morteza/ezTrading/data", 0777);
                snprintf(full_path, 128, "/Users/morteza/ezTrading/data/%s", path);
            }

            if (filenames.contains(full_path)) {
                throw "File was already in use";
            }
            bool truncate = false;
            void *mmap = map_file(full_path, MAX_HISTORY_SIZE, truncate);
            if (!mmap) {
                throw "Could not map file";
            }
            data.reconstruct_from_file((char*) mmap, truncate, (MAX_HISTORY_SIZE - 32) / sizeof(T));
            if (truncate) {
                data.push_back(Data { T(), system_clock::now() });
            }
            return true;
        }
        return false;
    }

    inline const T *operator->() const {
        std::shared_lock lock(hist_mutex);
        return &data.back().data;
    }

    inline const T &operator*() const {
        std::shared_lock lock(hist_mutex);
        return data.back().data;
    }

    inline T &mut(Worker *job) {
        emit_later(job);
        return mut_ref();
    }

    inline T &mut() { // Does not emit
        return mut_ref();
    }

    const T* operator[](system_clock::duration delta) const {
        return &history(delta);
    }

    const T& history(system_clock::duration delta) const {
        std::shared_lock lock(hist_mutex);
        auto low = std::lower_bound(data.begin(), data.end(), system_clock::now() + delta);
        return low->data;
    }

    inline void push_to_history(const T &val, system_clock::time_point timestamp) {
        std::unique_lock lock(hist_mutex);
        data.push_back(Data { val, timestamp });
    }
};

template <class T>
struct __is_signal {
    static const bool value = false;
};

template <class T>
struct __is_signal<Signal<T>> {
    static const bool value = true;
};

struct __on_changed_event {
    template <class T>
    static bool changed(const T &v, Worker *job) {
        if constexpr (__is_signal<T>::value) {
            return v.changed(job);
        } else {
            return v;
        }
    }
};