#pragma once
#include <vector>
#include <sys/mman.h>
#include <chrono>
#include <cstring>

template <class T>
using Vec = std::vector<T>;


/// @brief This vector class can be used between processes and reconstruct from
/// the shared memory. It can also reconstruct from a Rust Vec object. This is
/// useful in Signal objects. An external API can reconstruct the time series
/// shared vector when a new data arrives. Note that the shared memory for the
/// vectors should not change (reserve max in DataSrv).
template <class T>
class __shared_vector {
    std::vector<T> *vec;
    static_assert(sizeof(T) < 8160, "Cannot store objects larger than 8160 bytes.");
public:
    __shared_vector() : vec(new std::vector<T>()) {}

    /// @brief Assign a shared vector from file. The layout of the objects in a
    /// file is as follows:
    ///      | size in bytes (8B) |  cap in bytes (8B) | Items ... |
    /// @param base: a pointer to the memory mapped file
    /// @param trunc: truncate the vector
    /// @param capacity: maximum size of the vector
    void reconstruct_from_file(char *base, bool trunc, size_t capacity) {
        size_t *pointers = (size_t*)base;
        if (trunc) {
            std::memset(base, 0, 32);
            pointers[1] = pointers[2] = 32;
            pointers[3] = 32 + capacity * sizeof(T);
        }
        size_t old_base = pointers[0];
        size_t new_base = (size_t)base;
        pointers[0] = new_base;
        pointers[1] = pointers[1] - old_base + new_base;
        pointers[2] = pointers[2] - old_base + new_base;
        pointers[3] = pointers[3] - old_base + new_base;
        msync(base, 32, MS_SYNC | MS_INVALIDATE);
        vec = (std::vector<T>*)(&pointers[1]);
    }

    void reconstruct(T* begin, T* end, size_t cap) {
        void **vec_ptr = (void**)(vec);
        vec_ptr[0] = (void*)(begin);
        vec_ptr[1] = (void*)(end);
        vec_ptr[2] = (void*)(begin + cap);
    }

    typename std::vector<T>::iterator begin() {
        return vec->begin();
    }

    typename std::vector<T>::iterator end() {
        return vec->end();
    }

    T &back() {
        return vec->back();
    }

    const T &back() const {
        return vec->back();
    }

    void push_back(const T &val) {
        void **vec_ptr = (void**)(vec);
        if (vec_ptr[0] == vec_ptr[2]) {
            vec->reserve(10);
        }
        if (vec_ptr[1] == vec_ptr[2]) {
            shift(1);
        }
        vec->push_back(val);
    }

    void push_back(T &&val) {
        void **vec_ptr = (void**)(vec);
        if (vec_ptr[0] == vec_ptr[2]) {
            vec->reserve(10);
        }
        if (vec_ptr[1] == vec_ptr[2]) {
            shift(1);
        }
        vec->push_back(val);
    }

    /// @brief Shift items to left to make room for new items. Discards the
    ///        first `count` items.
    /// @param count: Number of items to be discarded from the beginning of the
    ///               list.
    /// @note Synchronization is required.
    void shift(size_t count) {
        assert(count <= size() && "cannot shift more than size");
        void **vec_ptr = (void**)(vec);
        std::memcpy(vec_ptr[0], (char*)vec_ptr[0]+(count*sizeof(T)), (vec->size()-count)*sizeof(T));
        (T*&)(vec_ptr[1]) -= count;
    }

    T &operator[](size_t idx) {
        return vec->operator[](idx);
    }

    const T &operator[](size_t idx) const {
        return vec->operator[](idx);
    }

    size_t size() const {
        return vec->size();
    }
    
    bool empty() const {
        return vec->empty();
    }
};
