#pragma once

#include <thread>
#include <vector>
#include <queue>
#include <unordered_set>
#include <iomanip>
#include <cstdlib>
#include <chrono>
#include <mutex>

using std::chrono::system_clock;

class Worker;


struct WorkerCandidate {
    Worker *job;
    int version;
    WorkerCandidate(Worker *job=nullptr, int version=0) : job(job), version(version) {}
};

struct ScheduledWorker: public WorkerCandidate {
    system_clock::time_point scheduled_time;

    friend bool operator< (ScheduledWorker const& lhs, ScheduledWorker const& rhs) {
        return lhs.scheduled_time > rhs.scheduled_time;
    }
};

class Scheduler {
private:
    std::mutex sig;
    std::mutex pool_m;
    std::vector<Worker*> jobs;
    std::thread *thread, *timer_thread;
    std::atomic_bool working;

    std::chrono::milliseconds deadline, interval;
    system_clock::time_point epoch;

    std::mutex queue_m;
    std::queue<WorkerCandidate> event_queue;
    WorkerCandidate current_worker;

    std::mutex schedule_jobs_mutex;
    std::atomic_bool has_schedule_job;
    std::priority_queue<ScheduledWorker> scheduled_jobs;
    system_clock::time_point earliest_signal_time;


    inline Worker *next_job();

public:
    static void worker(Scheduler*);
    static void timer(Scheduler*);

    void start() {
        thread = new std::thread(Scheduler::worker, this);
        timer_thread = new std::thread(Scheduler::timer, this);
    }

    size_t job_count() {
        return jobs.size();
    }

    Scheduler(size_t deadline = 5000):
    current_worker(), working(true),
    deadline(deadline), interval(250),
    earliest_signal_time(system_clock::time_point::max()) {
        start();
    }

    void register_worker(Worker *worker) {
        pool_m.lock();
        jobs.push_back(worker);
        #ifdef DEBUG
        printf("Job %p is registered\n", worker);
        #endif
        pool_m.unlock();
    }

    void stop() {
        working.store(false);
        release();
    }

    void join() {
        if (thread) thread->join();
        if (timer_thread) timer_thread->join();
    }

    inline void acquire() {
        sig.lock();
    }

    inline void release() {
        sig.unlock();
    }

    void submit(Worker* job);
    void schedule(Worker* job, system_clock::time_point time);

    void kill(Worker *job);
    inline void reset_timer();

    void run();
    void start_timer();
};

class WorkerComp;
class SignalCore;
class Account;

template <class T> class Signal;

class Worker {
protected:
    size_t id;
    std::mutex sig;
    Scheduler *scheduler;
    std::thread *thread;
    std::atomic_bool running;
    system_clock::time_point __cycle;
    system_clock::time_point __prev_cycle;
    system_clock::time_point __next_cycle;
    system_clock::time_point __last_scheduled_cycle;
    std::unordered_set<SignalCore*> emit_list;
    bool is_preprocessing;
    int version;

    friend class WorkerComp;
    friend class Scheduler;
    friend class SignalCore;
    friend class Account;

public:
    uint32_t __run_id;
    
    Worker(uint32_t run_id = 100): __run_id(run_id), is_preprocessing(false) {
        static size_t last_id = 1;
        id = last_id++;
        running = false;
        __prev_cycle = __cycle = system_clock::now();
        __next_cycle = system_clock::time_point::max();
        __last_scheduled_cycle = system_clock::time_point::max();
        version = 1;
        sig.lock();
    }

    void setup(Scheduler *scheduler) {
        this->scheduler = scheduler;
        scheduler->register_worker(this);
    }

    void emit_later(SignalCore* sig) {
        emit_list.insert(sig);
    }

    void emit_signals();

    void stop() {
        running.store(false);
    }

    inline void die() {
        bool was_running = true;
        if (running.compare_exchange_strong(was_running, false)) {
            version++;
            #ifdef DEBUG
            printf("\033[1;31mI am suppose to be killed :( %p\033[0m\n", this);
            #endif
            pthread_cancel(thread->native_handle());
        }
    }

    inline bool is_running() {
        return running.load();
    }

    void join() {
        thread->join();
    }

    inline void release() {
        sig.unlock();
    }

    void set_next_cycle(system_clock::time_point next) {
        __next_cycle = next;
    }

    inline void acquire() {
        sig.lock();
        #ifdef DEBUG
        printf("\033[0;32mReceived the firing signal... %p\033[0m\n", this);
        #endif
        __prev_cycle = __cycle;
        __cycle = system_clock::now();
    }

    inline void reschedule() {
        if (__next_cycle != system_clock::time_point::max() && __next_cycle != __last_scheduled_cycle) {
            if ( __next_cycle < system_clock::now() ) {
                // warning: it took too long
            }
            __last_scheduled_cycle = __next_cycle;
            scheduler->schedule(this, __next_cycle);
        }
    }

    inline void resubmit() {
        emit_signals();
        scheduler->submit(this);
        scheduler->release();
    }

    inline void notify() {
        emit_signals();
        reschedule();
        scheduler->release();
    }

    inline void sleep() {
        scheduler->release();
        sig.lock();
        #ifdef DEBUG
        printf("\033[0;32mPutting %p to sleep\033[0m\n", this);
        #endif
    }

    inline void wakeup() {
        sig.unlock();
        #ifdef DEBUG
        printf("\033[0;32mWaking up %p\033[0m\n", this);
        #endif
        scheduler->submit(this);
        scheduler->release();
    }
};

struct WorkerComp
{
    bool operator() ( const Worker* s, size_t i ) const { return s->id < i; }
    bool operator() ( size_t i, const Worker* s ) const { return i < s->id; }
};
