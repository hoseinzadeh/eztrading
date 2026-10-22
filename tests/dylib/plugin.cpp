/// This is the the plugin to load

#include <thread>
#include <chrono>
#include <atomic>

#define MAX (100 << 11)

class Worker {
    int id;
    std::thread *thread;
    char chunk[MAX];
public:
    void start();
    void die();

    Worker(int id): id(id) {
        thread = new std::thread(&Worker::start, this);
    }

    void join() {
        if (thread) thread->join();
    }
};

std::atomic<std::atomic_int32_t*> round (nullptr);

void Worker::start() {
    while (true) {
        // for (int i=0; i < MAX; i+=100) {
        //     chunk[i] += 'A';
        // }
        if (round.load())
            printf("[PLUGIN %d] round %d [%lu]\n", id, round.load()->fetch_add(1), sizeof(*this));
        std::this_thread::sleep_for(std::chrono::seconds(1));
    }
}

void Worker::die() {
    pthread_cancel(thread->native_handle());
        printf("[PLUGIN %d] died.\n", id);
}

extern "C" void *run(int id, void *counter) {
    std::atomic_int32_t*cur = nullptr;
    round.compare_exchange_strong(cur, (std::atomic_int32_t*)counter);
    Worker *w = new Worker(id);
    return w;
}

extern "C" void stop(void *p) {
    Worker *w = (Worker*) p;
    w->die();
}