#include <atomic>

template<typename T, const size_t N=128>
class atomic_queue
{
private:
    struct node
    {
        T data;
        std::atomic<node*> next;
        node(): next(nullptr) {}
        void init(T const& data_) {
            data = data_;
            next.store(nullptr);
        }
    };
    std::atomic<node*> head;
    std::atomic<node*> tail;

    template<class U>
    struct fixed_size_allocator_t {
        struct link {
            U data;
            link *next;
        };
        link *buffer;
        std::atomic<link*> head;
        std::atomic_int32_t size;
        std::counting_semaphore<1> wait_for_free;
        fixed_size_allocator_t() : head(nullptr), wait_for_free(0), size(N) {
            buffer = new link[N]();
            for (int i=0; i<N-1; i++) {
                buffer[i].next = &buffer[i+1];
            }
            buffer[N-1].next = nullptr;
            head.store(buffer);
        }
        U *alloc() {
            return new U;
            // TODO: Fix the atomic ring buffer allocator
            while (true) {
                auto p = head.load();
                while (p && !head.compare_exchange_weak(p, p->next)) { }
                if (!p) {
                    printf("\033[1;32mQueue is full (%d)\033[0m\n", size.load());
                    wait_for_free.acquire();
                    continue; // Retry
                }
                size.fetch_sub(1);
                p->next = nullptr;
                return &p->data;
            }
        }
        void free(U *d) {
            delete d; return;
            // TODO: Fix the atomic ring buffer allocator
            link *p = (link*)d;
            p->next = head.load();
            while (!head.compare_exchange_weak(p->next, p)) {}
            size.fetch_add(1);
            if (!p->next) {
                printf("\033[0;33mOne slot became available\033[0m\n");
                wait_for_free.release();
            }
        }
    };

    fixed_size_allocator_t<node> alloc;
public:
    atomic_queue() : head(nullptr), tail(nullptr) {}

    // Returns true if the queue was empty before
    bool push(T const& data)
    {
        node* new_node = alloc.alloc();
        new_node->init(data);
        node* current = nullptr;
        node* old_tail = tail.load();
        while(old_tail && !old_tail->next.compare_exchange_weak(current, new_node)) {
          old_tail = current;
          current = nullptr;
        }
        tail.compare_exchange_strong(old_tail, new_node);
        return head.compare_exchange_strong(current, new_node);
    }
    T *pop()
    {
        node* old_head=head.load();
        while(old_head &&
            !head.compare_exchange_weak(old_head,old_head->next)) { }
        auto res = old_head ? &old_head->data : nullptr;
        if (old_head) alloc.free(old_head);
        return res;
    }
    bool empty() {
        return head.load() == nullptr;
    }
    inline int32_t size() {
        return N - alloc.size.load();
    }
};