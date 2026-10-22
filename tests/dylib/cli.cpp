/// This is a cli interface to load, call, and unload a library
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <dlfcn.h>
#include <vector>
#include<unistd.h> 

struct obj_t {
    void *handle;
    void *worker;
};

int round = 100;
std::atomic_int32_t counter(0);
int main(int argc, char *argv[]) {

    for (int round = 0; round<3; round++) {
        std::vector<obj_t> handles;

        for (int id = 100; id < 300; id+=100) {
            obj_t obj;
            void *(*run)(int,void*);
            obj.handle = dlopen("./libplugin.dylib", RTLD_LAZY);

            if (!obj.handle) {
                /* fail to load the library */
                fprintf(stderr, "Error: %s\n", dlerror());
                return EXIT_FAILURE;
            }

            *(void**)(&run) = dlsym(obj.handle, "run");
            if (!run) {
                /* no such symbol */
                fprintf(stderr, "Error: %s\n", dlerror());
                dlclose(obj.handle);
                return EXIT_FAILURE;
            }

            obj.worker = run(id, &counter);
            handles.push_back(obj);
        }

        sleep(5);

        for (auto obj: handles) {
            void (*stop)(void *);
            *(void**)(&stop) = dlsym(obj.handle, "stop");
            if (!stop) {
                /* no such symbol */
                fprintf(stderr, "Error: %s\n", dlerror());
                dlclose(obj.handle);
                return EXIT_FAILURE;
            }
            stop(obj.worker);

            dlclose(obj.handle);
        }

        printf("Round %d is done.\n", round+1);
    }

    return EXIT_SUCCESS;
    return 0;
}