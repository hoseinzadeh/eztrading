
#include <iostream>
#include <semaphore>
#include <thread>
#include <unistd.h>
#include <cmath>
#include <vector>

class Worker;

class Scheduler {
private:
    std::counting_semaphore<1> sig;
    std::vector<Worker*> queue;
    std::thread *thread;
    int rounds, id;
    friend class Worker;
public:
    Scheduler(): sig(0), thread(NULL), rounds(0) {
        sig.release();
    }

    void setup(int id, int rounds) {
        this->rounds = rounds;
        this->id = id;
    }

    void add_worker(Worker *worker) {
        queue.push_back(worker);
    }

    void start() {
        if (queue.empty()) return;
        thread = new std::thread(Scheduler::worker, this);
    }

    void join() {
        if (thread) thread->join();
    }

    void release() {
        sig.release();
    }

    static void worker(Scheduler *me);
};

class Worker {
private:
    std::counting_semaphore<1> sig;
    Scheduler *scheduler;
    std::thread *thread;
    int rounds, id;
public:
    Worker() : sig(0) {}

    void setup(Scheduler *scheduler, int rounds, int id) {
        this->rounds = rounds;
        this->scheduler = scheduler;
        this->id = id;
        scheduler->add_worker(this);
    }

    void start() {
        thread = new std::thread(Worker::worker, this);
    }

    void join() {
        thread->join();
    }

    void release() {
        sig.release();
    }

    void setup(Scheduler *pinger) {
        this->scheduler = pinger;
    }

    static void worker(Worker *me);
};

void Scheduler::worker(Scheduler *me) {
    int round = 0;
    int idx = 0;
    int count = me->queue.size();
    int rounds = me->rounds * count;
    while(round <= rounds) {
        me->sig.acquire();
        ++round;
        me->queue[idx]->release();
        idx = (idx+1) % count;
    }
}

void Worker::worker(Worker *me) {
    int round = 0;
    while(round < me->rounds) {
        me->sig.acquire();
        sleep(1);
        round++;
        me->scheduler->release();
    }
}

int main(int argc, char *argv[]) {
    if (argc < 4) {
        std::cout << "usage: " << argv[0]
                  << " [schedulers] [workers] [rounds]" << std::endl;
        return 0;
    }

    int rounds = atoi(argv[3]);
    int num_schdlr = atoi(argv[1]);
    int num_workrs = atoi(argv[2]);
    int partition = ceil((double) num_workrs/num_schdlr);

    assert(num_workrs >= num_schdlr);

    std::cout << "Expected duration: " << (rounds * partition)
              << " seconds (ideal time: "
              << ((double) rounds * num_workrs / num_schdlr) << " seconds)\n";

    Scheduler *schedulers = new Scheduler[num_schdlr];
    Worker *workers = new Worker[num_workrs];

    // Setup
    double rem = num_workrs;
    for (int i=0; i < num_schdlr; i++) {
        if (i == num_schdlr-1) {
            schedulers[i].setup(i, rounds);
        } else {
            schedulers[i].setup(i, rounds);
        }
        rem -= partition;
    }

    for (int i=0; i < num_workrs; i++) {
        workers[i].setup(&schedulers[i % num_schdlr], rounds, i);
    }

    auto start = std::chrono::system_clock::now();

    // Run in parallel
    for (int i=0; i < num_schdlr; i++) {
        schedulers[i].start();
    }

    for (int i=0; i < num_workrs; i++) {
        workers[i].start();
    }

    // Join
    for (int i=0; i < num_schdlr; i++) {
        schedulers[i].join();
    }

    for (int i=0; i < num_workrs; i++) {
        workers[i].join();
    }

    std::chrono::duration<double> dur = std::chrono::system_clock::now() - start;
    std::cout << "Duration: " << dur.count() << " seconds" << '\n';

}