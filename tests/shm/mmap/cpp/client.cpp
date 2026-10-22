#include <sys/mman.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <chrono>
#include <thread>

#define handle_error(msg)   \
    do                      \
    {                       \
        perror(msg);        \
        exit(EXIT_FAILURE); \
    } while (0)

int main(int argc, char *argv[])
{
    if (argc < 2)
    {
        fprintf(stderr, "Need at least one argument to write to the file\n");
        exit(EXIT_FAILURE);
    }
    
    const char *text = argv[1];
    int offset = argc == 3 ? atoi(argv[2]) : 0;
    printf("Will write text '%s'\n", text);
        
    /* Open a file for writing.
     *  - Creating the file if it doesn't exist.
     *  - Truncating it to 0 size if it already exists. (not really needed)
     *
     * Note: "O_WRONLY" mode is not sufficient when mmaping.
     */
    
    const char *filepath = argv[1];

    int fd = open(filepath, O_RDWR | O_CREAT, (mode_t)0600);
    
    if (fd == -1)
    {
        perror("Error opening file for writing");
        exit(EXIT_FAILURE);
    }
    

    // Now the file is ready to be mmapped.
    char *map = (char*)mmap(0, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (map == MAP_FAILED)
    {
        close(fd);
        perror("Error mmapping the file");
        exit(EXIT_FAILURE);
    }

    srand(time(0));
    int idx = 0;
    while (true) {
        map[offset + idx % 8] = 'A' + (idx % 26);
        // cacheflush(map, 64, DCACHE);
        // msync(map, 64, MS_ASYNC);
        std::this_thread::sleep_for(std::chrono::milliseconds(rand()%200));
        idx++;
    }
    
    // Don't forget to free the mmapped memory
    if (munmap(map, 4096) == -1)
    {
        close(fd);
        perror("Error un-mmapping the file");
        exit(EXIT_FAILURE);
    }

    // Un-mmaping doesn't close the file, so we still need to do that.
    close(fd);
    
    return 0;
}