#include <scheduler.h>
#include <sig.h>

using std::chrono::system_clock;

void Scheduler::run() {
    while(working.load()) {
        acquire();
        if (!working.load()) break;
        Worker *job = next_job();
        if (job) {
            current_worker = WorkerCandidate(job, job->version);
            #ifdef DEBUG
            auto time = std::chrono::system_clock::now();
            auto c_t = system_clock::to_time_t(time);
            char mbstr[100];
            if (std::strftime(mbstr, sizeof(mbstr), "%A %c", std::localtime(&c_t))) {
                printf("\033[1;33m%s Firing %p ...\033[0m\n", mbstr, job);
            }
            #endif
            reset_timer();
            job->release();
        } else {
            current_worker = WorkerCandidate(nullptr, 0);
        }
    }
}

void Scheduler::worker(Scheduler *sch) {
    sch->run();
}

inline Worker *Scheduler::next_job() {
    schedule_jobs_mutex.lock();
    if (earliest_signal_time <= system_clock::now()) {
        #ifdef DEBUG
        auto c_t = system_clock::to_time_t(earliest_signal_time);
        char mbstr[100];
        if (std::strftime(mbstr, sizeof(mbstr), "%A %c", std::localtime(&c_t))) {
            printf("\033[1;36mearliest_signal_time = %s\033[0m\n", mbstr);
        }
        #endif
        bool not_empty = true;
        if (has_schedule_job.compare_exchange_strong(not_empty, false)) {

            Worker *job = nullptr;
            ScheduledWorker worker = scheduled_jobs.top();
            for (; !scheduled_jobs.empty(); worker = scheduled_jobs.top()) {
                if (worker.job->version == worker.version) {
                    job = worker.job;
                    break;
                }
                scheduled_jobs.pop();
            }

            if (job && worker.scheduled_time >= earliest_signal_time) {
                scheduled_jobs.pop();
                if (!scheduled_jobs.empty()) {
                    ScheduledWorker top = scheduled_jobs.top();
                    earliest_signal_time = top.scheduled_time;
                    has_schedule_job.store(true);
                } else {
                    earliest_signal_time = system_clock::time_point::max();
                }
            } else if (!job) {
                earliest_signal_time = system_clock::time_point::max();
            } else {
                job = nullptr;
            }

            schedule_jobs_mutex.unlock();
            if (job) {
                #ifdef DEBUG
                printf("\033[1;36mcurrent job is %p (ver=%d, curr=%d, scheduled)\033[0m\n", worker.job, worker.version, worker.job->version);
                #endif
                return job;
            }
        }
    }
    schedule_jobs_mutex.unlock();

    const std::lock_guard<std::mutex> lock(queue_m);
    WorkerCandidate worker;
    while (!event_queue.empty()) {
        worker = event_queue.front();
        event_queue.pop();
        #ifdef DEBUG
        printf("\033[1;36mcurrent job is %p (ver=%d, curr=%d)\033[0m\n", worker.job, worker.version, worker.job->version);
        #endif
        if (worker.job->version == worker.version) {
            return worker.job;
        }
    }

    #ifdef DEBUG
    printf("\033[1;36mNo job is popped\033[0m\n");
    #endif
    return nullptr;
}

void Scheduler::submit(Worker* job) {
    if (!job->is_running()) return;
    const std::lock_guard<std::mutex> lock(queue_m);
    #ifdef DEBUG
    printf("Job %p is submitted (ver = %d)\n", job, job->version);
    #endif
    event_queue.push(WorkerCandidate {job, job->version});
    if (event_queue.size() == 1) release();
}

void Scheduler::schedule(Worker* job, system_clock::time_point time) {
    if (!job->is_running()) return;
    #ifdef DEBUG
    auto c_t = system_clock::to_time_t(time);
    char mbstr[100];
    if (std::strftime(mbstr, sizeof(mbstr), "%A %c", std::localtime(&c_t))) {
        printf("Job %p is scheduled for %s\n", job, mbstr);
    }
    #endif
    schedule_jobs_mutex.lock();
    scheduled_jobs.push(ScheduledWorker{ WorkerCandidate {job, job->version}, time});
    if (time < earliest_signal_time) earliest_signal_time = time;
    has_schedule_job.store(true);
    schedule_jobs_mutex.unlock();
}

void Scheduler::start_timer() {
    while (working.load()) {
        Worker *job = current_worker.job;
        std::this_thread::sleep_for(interval);
        if (!job) {
            schedule_jobs_mutex.lock();
            if (earliest_signal_time <= system_clock::now()) {
                release();
            }
            schedule_jobs_mutex.unlock();
            continue;
        }
        auto current_time = system_clock::now();
        std::chrono::duration<double> diff = current_time - epoch;
        if (diff > deadline) {
            kill(job);
            reset_timer();
        }
    }
}

void Scheduler::timer(Scheduler *sch) {
    sch->start_timer();
}

inline void Scheduler::reset_timer() {
    epoch = system_clock::now();
}

void Scheduler::kill(Worker *job) {
    if (job && job == current_worker.job && job->version == current_worker.version) {
        pool_m.lock(); // pause scheduling other jobs
        job->die();

        auto j = std::lower_bound(jobs.begin(), jobs.end(), job->id, WorkerComp{});
        if (j != jobs.end()) {
            // The job may be already killed
            jobs.erase(j);
        }
        pool_m.unlock();
        release();
    }
}

void Worker::emit_signals() {
    for (auto sig: emit_list) {
        sig->emit(scheduler);
    }
    emit_list.clear();
}

/*********** Scheduler Interface ************/

#ifdef SCHEDULER_INTERFACE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <dlfcn.h>
#include <vector>
#include <unistd.h>
#include <eztrading.h>

struct Plugin {
    void *handle;
    __contents::list contents;
    std::vector<sml::machine*> machines;

    sml::machine *(*launch)(uint32_t user_id, Scheduler *scheduler,
        const char *model_name, char *machine_json);

    void unload() {
        for (auto m: machines) {
            m->die();
        }
        dlclose(handle);
    }
};

extern "C" void *new_scheduler() {
    return new Scheduler;
}

extern "C" bool load(void *handle, Plugin **pluginp) {
    char *error;
    if (!*pluginp) *pluginp = new Plugin;
    Plugin *plugin = *pluginp;
    plugin->handle = handle;

    if (!handle) {
        /* fail to load the library */
        fprintf(stderr, "%s\n", dlerror());
        return false;
    }

    void (*contents)(__contents::list *);

    *(void**)(&contents) = dlsym(handle, "contents");
    if (!contents) {
        /* no such symbol */
        fprintf(stderr, "%s\n", dlerror());
        dlclose(handle);
        return false;
    }
    dlerror();

    *(void**)(&plugin->launch) = dlsym(handle, "launch");
    if (!plugin->launch) {
        /* no such symbol */
        fprintf(stderr, "%s\n", dlerror());
        dlclose(handle);
        return false;
    }
    dlerror();

    contents(&plugin->contents);
    return true;
}

extern "C" void delete_plugin(Plugin *plugin) {
    if (plugin) delete plugin;
}

extern "C" void unload(Plugin *plugin) {
    plugin->unload();
}

extern "C" void join(Scheduler *sch) {
    sch->join();
}

extern "C" void stop(Scheduler *sch) {
    sch->stop();
}

extern "C" void plugin_contents(const Plugin *plugin, char **json, size_t *size) {
    auto res = plugin->contents.to_json();
    *size = res.size();
    *json = new char[*size];
    strcpy(*json, res.data());
}

extern "C" sml::machine *launch_machine(const Plugin *plugin, uint32_t user_id,
        Scheduler *scheduler, const char *model_name, char *machine_json) {
    printf("plugin: %p, user: %u, sch: %p, model: %s\n", plugin, user_id, scheduler, model_name);
    return plugin->launch(user_id, scheduler, model_name, machine_json);
}

extern "C" size_t job_count(Scheduler *scheduler) {
    return scheduler->job_count();
}

#endif