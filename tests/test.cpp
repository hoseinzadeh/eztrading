#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <dlfcn.h>
#include <vector>
#include <unistd.h> 
#include <eztrading.h>

struct Plugin {
    const char *path;
    void *handle;
    __contents::list contents;

    void unload() {
        dlclose(handle);
    }

    sml::machine *(*launch)(uint32_t user_id, Scheduler *scheduler,
        const char *model_name, __contents::machine *params);
};

Plugin load(const char *path) {
    Plugin lib;
    lib.handle = dlopen(path, RTLD_LAZY);

    if (!lib.handle) {
        /* fail to load the library */
        fprintf(stderr, "Error: %s\n", dlerror());
        exit(EXIT_FAILURE);
    }

    void (*contents)(__contents::list *);

    *(void**)(&contents) = dlsym(lib.handle, "contents");
    if (!contents) {
        /* no such symbol */
        fprintf(stderr, "Error: %s\n", dlerror());
        dlclose(lib.handle);
        exit(EXIT_FAILURE);
    }

    *(void**)(&lib.launch) = dlsym(lib.handle, "launch");
    if (!contents) {
        /* no such symbol */
        fprintf(stderr, "Error: %s\n", dlerror());
        dlclose(lib.handle);
        exit(EXIT_FAILURE);
    }

    lib.path = path;
    contents(&lib.contents);

    return lib;
}

void unload(const Plugin &lib) {

}

void reload(Plugin &lib) {
}

void restart(const Plugin &lib) {}

int main(int argc, char *argv[]) {
    if (argc < 3) {
        printf("Usage:\n  %s [lib] [machine]\n", argv[0]);
        printf("Example:\n  %s example.so example::TrailingStopWithProfit\n", argv[0]);
        return 0;
    }
    auto scheduler = new Scheduler;
    printf("Loading lib...\n");
    Plugin plugin = load(argv[1]);
    std::string j = plugin.contents.to_json().c_str();
    char *d = j.data();
    printf("Machines: %s\n", d);
    auto c = __contents::list::from_json(d);
    printf("\nReconstructed: %s\n", c.to_json().c_str());

    sml::machine *m1 = plugin.launch(100, scheduler, argv[2], nullptr);
    sml::machine *m2 = plugin.launch(110, scheduler, argv[2], nullptr);
    sml::machine *m3 = plugin.launch(120, scheduler, argv[2], nullptr);

    sleep(3);
    printf("Killing m1 ");
    m1->die();
    printf("[done]\n");
    sleep(3);
    printf("Killing m2 ");
    m2->die();
    printf("[done]\n");
    sleep(3);
    printf("Killing m3 ");
    m3->die();
    printf("[done]\n");
    plugin.unload();
    printf("Done\n");
}
