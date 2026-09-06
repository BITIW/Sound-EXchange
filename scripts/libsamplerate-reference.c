/* Minimal validation-only frontend for libsamplerate's public simple API.
 *
 * The SeX signal path never links this file. It exists solely so the external
 * validation harness can exercise an installed libsamplerate shared library
 * even when a distribution does not ship sndfile-resample or development
 * headers.
 */

#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    const float *data_in;
    float *data_out;
    long input_frames;
    long output_frames;
    long input_frames_used;
    long output_frames_gen;
    int end_of_input;
    double src_ratio;
} SRC_DATA;

typedef int (*src_simple_function)(SRC_DATA *, int, int);
typedef const char *(*src_strerror_function)(int);
typedef const char *(*src_get_version_function)(void);

static long parse_positive_long(const char *text, const char *name) {
    char *end = NULL;
    errno = 0;
    const long value = strtol(text, &end, 10);
    if (errno != 0 || end == text || *end != '\0' || value <= 0) {
        fprintf(stderr, "invalid %s: %s\n", name, text);
        exit(2);
    }
    return value;
}

static void *checked_calloc(size_t count, size_t width) {
    if (width != 0 && count > SIZE_MAX / width) {
        fputs("reference buffer size overflow\n", stderr);
        exit(2);
    }
    void *result = calloc(count, width);
    if (result == NULL) {
        fputs("reference buffer allocation failed\n", stderr);
        exit(2);
    }
    return result;
}

static void read_exact(float *samples, size_t count) {
    size_t position = 0;
    while (position < count) {
        const size_t read = fread(samples + position, sizeof(*samples), count - position, stdin);
        if (read == 0) {
            if (ferror(stdin)) {
                perror("reading reference input");
            } else {
                fputs("reference input ended early\n", stderr);
            }
            exit(2);
        }
        position += read;
    }
    float extra = 0.0F;
    if (fread(&extra, sizeof(extra), 1, stdin) != 0) {
        fputs("reference input contains more frames than declared\n", stderr);
        exit(2);
    }
    if (ferror(stdin)) {
        perror("checking reference input length");
        exit(2);
    }
}

int main(int argc, char **argv) {
    if (argc == 2 && (strcmp(argv[1], "--version") == 0 || strcmp(argv[1], "--soxr-version") == 0)) {
        const int soxr = strcmp(argv[1], "--soxr-version") == 0;
        const char *name = soxr ? "libsoxr.so.0" : "libsamplerate.so.0";
        void *library = dlopen(name, RTLD_NOW | RTLD_LOCAL);
        if (library == NULL) {
            fprintf(stderr, "cannot load %s: %s\n", name, dlerror());
            return 2;
        }
        void *version_symbol = dlsym(library, soxr ? "soxr_version" : "src_get_version");
        if (version_symbol == NULL || sizeof(version_symbol) != sizeof(src_get_version_function)) {
            fputs("reference version symbol is unavailable or ABI-incompatible\n", stderr);
            return 2;
        }
        src_get_version_function get_version = NULL;
        memcpy(&get_version, &version_symbol, sizeof(get_version));
        puts(get_version());
        return dlclose(library) == 0 ? 0 : 2;
    }
    if (argc != 5) {
        fprintf(stderr,
                "usage: %s INPUT_RATE OUTPUT_RATE CHANNELS INPUT_FRAMES\n"
                "       %s --version\n",
                argv[0], argv[0]);
        return 2;
    }
    const long input_rate = parse_positive_long(argv[1], "input rate");
    const long output_rate = parse_positive_long(argv[2], "output rate");
    const long channels = parse_positive_long(argv[3], "channel count");
    const long input_frames = parse_positive_long(argv[4], "input frame count");
    if (channels > INT_MAX) {
        fputs("reference channel count exceeds int\n", stderr);
        return 2;
    }

    const double ratio = (double)output_rate / (double)input_rate;
    const double estimated = ceil((double)input_frames * ratio);
    if (!isfinite(estimated) || estimated > (double)(LONG_MAX - 4096)) {
        fputs("reference output frame count overflow\n", stderr);
        return 2;
    }
    const long output_capacity = (long)estimated + 4096;
    if ((unsigned long)input_frames > SIZE_MAX / (unsigned long)channels ||
        (unsigned long)output_capacity > SIZE_MAX / (unsigned long)channels) {
        fputs("reference interleaved sample count overflow\n", stderr);
        return 2;
    }
    const size_t input_samples = (size_t)input_frames * (size_t)channels;
    const size_t output_samples = (size_t)output_capacity * (size_t)channels;
    float *input = checked_calloc(input_samples, sizeof(*input));
    float *output = checked_calloc(output_samples, sizeof(*output));
    read_exact(input, input_samples);

    void *library = dlopen("libsamplerate.so.0", RTLD_NOW | RTLD_LOCAL);
    if (library == NULL) {
        fprintf(stderr, "cannot load libsamplerate.so.0: %s\n", dlerror());
        return 2;
    }
    void *simple_symbol = dlsym(library, "src_simple");
    void *strerror_symbol = dlsym(library, "src_strerror");
    if (simple_symbol == NULL || strerror_symbol == NULL ||
        sizeof(simple_symbol) != sizeof(src_simple_function) ||
        sizeof(strerror_symbol) != sizeof(src_strerror_function)) {
        fputs("libsamplerate public symbols are unavailable or ABI-incompatible\n", stderr);
        return 2;
    }
    src_simple_function src_simple_call = NULL;
    src_strerror_function src_error_text = NULL;
    memcpy(&src_simple_call, &simple_symbol, sizeof(src_simple_call));
    memcpy(&src_error_text, &strerror_symbol, sizeof(src_error_text));

    SRC_DATA data = {
        .data_in = input,
        .data_out = output,
        .input_frames = input_frames,
        .output_frames = output_capacity,
        .input_frames_used = 0,
        .output_frames_gen = 0,
        .end_of_input = 1,
        .src_ratio = ratio,
    };
    /* SRC_SINC_BEST_QUALITY is the stable public enum value zero. */
    const int error = src_simple_call(&data, 0, (int)channels);
    if (error != 0) {
        fprintf(stderr, "libsamplerate failed: %s\n", src_error_text(error));
        return 1;
    }
    if (data.input_frames_used != input_frames) {
        fprintf(stderr, "libsamplerate consumed %ld of %ld input frames\n",
                data.input_frames_used, input_frames);
        return 1;
    }
    const size_t generated_samples = (size_t)data.output_frames_gen * (size_t)channels;
    if (fwrite(output, sizeof(*output), generated_samples, stdout) != generated_samples) {
        perror("writing reference output");
        return 2;
    }

    free(output);
    free(input);
    if (dlclose(library) != 0) {
        fputs("closing libsamplerate failed\n", stderr);
        return 2;
    }
    return 0;
}
