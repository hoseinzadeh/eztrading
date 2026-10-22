/* Filename: shm_read.c */
#include<stdio.h>
#include<sys/ipc.h>
#include<sys/shm.h>
#include<sys/types.h>
#include<string.h>
#include<errno.h>
#include<stdlib.h>
#include <unistd.h>
#include <ctime>
#include <chrono>

#define BUF_SIZE 1024
#define SHM_KEY 0x1234


struct Tick {
    float open,close,high,low;
    std::chrono::system_clock::time_point ts;
    char pad[8];
};

struct shmseg {
   int cnt;
   int complete;
   char buf[BUF_SIZE];
};

Tick syms[10000][3000];

int main(int argc, char *argv[]) {
   printf("size = %lu\n", sizeof(Tick));
   printf("size = %.2f MB\n", double(sizeof(syms))/1024.0/1024.0);
   int shmid;
   struct shmseg *shmp;
   shmid = shmget(SHM_KEY, sizeof(struct shmseg), 0644|IPC_CREAT);
   if (shmid == -1) {
      perror("Shared memory");
      return 1;
   }
   
   // Attach to the segment to get a pointer to it.
   shmp = (struct shmseg *)shmat(shmid, NULL, 0);
   if (shmp == (void *) -1) {
      perror("Shared memory attach");
      return 1;
   }
   
   /* Transfer blocks of data from shared memory to stdout*/
   while (shmp->complete != 1) {
      // printf("segment contains : \n\"%s\"\n", shmp->buf);
      if (shmp->cnt == -1) {
         perror("read");
         return 1;
      }
      printf("Reading Process: Shared Memory: Read %d bytes\n", shmp->cnt);
      sleep(3);
   }
   printf("Reading Process: Reading Done, Detaching Shared Memory\n");
   if (shmdt(shmp) == -1) {
      perror("shmdt");
      return 1;
   }
   printf("Reading Process: Complete\n");
   return 0;
}