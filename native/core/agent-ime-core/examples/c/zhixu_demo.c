/*
 * zhixu_demo.c — portable C11 console client for the zhixu pinyin IME core.
 *
 * Usage:
 *   zhixu-demo <absolute-shared-dictionary-directory> <absolute-private-user-directory> <ASCII-pinyin>
 *
 * Behavior:
 *   Creates one AimSession, feeds each ASCII letter of <ASCII-pinyin> as a
 *   separate AIM_KEY_TEXT key event (increasing non-zero delivery ids), prints
 *   the marked text and the first candidate page from the final composition,
 *   then sends AIM_KEY_SPACE and prints the committed text.
 *
 * Privacy note / 隐私说明:
 *   This demo intentionally displays the entered pinyin and the candidate
 *   choices in the console so the API can be observed. It never writes log
 *   files, never touches environment variables, and never performs any
 *   networking. The two directories are prepared by the caller and are only
 *   passed through to aim_session_create().
 *   本示例会在终端明文显示输入的拼音与候选结果，仅用于演示 API。
 *   程序不写日志文件、不修改环境变量、不做任何网络访问。
 *
 * Threading: single-threaded, strictly sequential calls; the core is never
 * re-entered and never called from multiple threads.
 *
 * Portability: C11, standard C library only; no macOS or GUI APIs.
 */

#include <inttypes.h>
#include <ctype.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "agent_ime_core.h"

#define ZHIXU_APP_ID "org.zhixu.sdk.demo"
#define ZHIXU_MAX_PINYIN_LEN 256u

/* Returned UTF-8 slices are byte lengths, not NUL terminated:
 * print with fwrite, never with %s or strlen. */
static void print_slice(const uint8_t *data, size_t len)
{
    if (data != NULL && len > 0) {
        fwrite(data, 1, len, stdout);
    } else {
        fputs("(empty)", stdout);
    }
}

static void print_call_error(const char *stage, int32_t c_ret, const AimResult *result)
{
    fprintf(stderr,
            "zhixu-demo: %s failed: C return=%" PRId32 ", result.status=%" PRId32 "\n",
            stage, c_ret, result->status);
}

static int pinyin_is_valid(const char *s, size_t *len_out)
{
    size_t n = strlen(s);
    if (n == 0 || n > ZHIXU_MAX_PINYIN_LEN) {
        return 0;
    }
    for (size_t i = 0; i < n; ++i) {
        if (s[i] < 'a' || s[i] > 'z') {
            return 0;
        }
    }
    *len_out = n;
    return 1;
}

static int absolute_path(const char *path)
{
#if defined(_WIN32)
    return (isalpha((unsigned char)path[0]) && path[1] == ':' && (path[2] == '/' || path[2] == '\\'))
        || (path[0] == '\\' && path[1] == '\\');
#else
    return path[0] == '/';
#endif
}

int main(int argc, char **argv)
{
    if (argc != 4) {
        fprintf(stderr,
                "usage: %s <absolute-shared-dictionary-directory> "
                "<absolute-private-user-directory> <ASCII-pinyin>\n",
                (argv[0] != NULL) ? argv[0] : "zhixu-demo");
        return 2;
    }

    const char *shared_dir = argv[1];
    const char *user_dir = argv[2];
    const char *pinyin = argv[3];

    if (!absolute_path(shared_dir) || !absolute_path(user_dir)) {
        fprintf(stderr, "zhixu-demo: both directory arguments must be absolute paths\n");
        return 2;
    }

    size_t pinyin_len = 0;
    if (!pinyin_is_valid(pinyin, &pinyin_len)) {
        fprintf(stderr,
                "zhixu-demo: pinyin must be 1..%u lower-case ASCII letters (a-z)\n",
                ZHIXU_MAX_PINYIN_LEN);
        return 2;
    }

    if (aim_core_abi_version() != AIM_CORE_ABI_VERSION) {
        fputs("zhixu-demo: header/library ABI mismatch\n", stderr);
        return 1;
    }
    printf("Zhixu %s (ABI %" PRIu32 ")\n", aim_core_version(), aim_core_abi_version());

    AimSession *session = aim_session_create(
        (const uint8_t *)shared_dir, strlen(shared_dir),
        (const uint8_t *)user_dir, strlen(user_dir),
        (const uint8_t *)ZHIXU_APP_ID, sizeof(ZHIXU_APP_ID) - 1);
    if (session == NULL) {
        fprintf(stderr,
                "zhixu-demo: session creation failed: dictionary/profile validation "
                "did not pass.\n"
                "Check that the shared dictionary directory and the private user "
                "directory exist, are accessible, and contain a valid zhixu "
                "dictionary/profile.\n");
        return 1;
    }

    int exit_code = 0;
    uint64_t delivery_id = 0;
    int32_t ret;

    AimKeyEvent event;
    AimResult result = {0};

    /* Feed the pinyin, one ASCII letter per key event, same session. */
    for (size_t i = 0; i < pinyin_len; ++i) {
        memset(&event, 0, sizeof(event));
        event.kind = AIM_KEY_TEXT;
        event.text = (const uint8_t *)&pinyin[i];
        event.text_len = 1;
        event.modifiers = 0;
        event.security_state = AIM_SECURITY_NON_SECURE;
        event.delivery_id = ++delivery_id; /* non-zero, strictly increasing */

        memset(&result, 0, sizeof(result));
        ret = aim_session_process(session, &event, &result);
        if (ret != 0 || result.status != 0) {
            print_call_error("pinyin key", ret, &result);
            aim_result_free(&result);
            exit_code = 1;
            goto destroy;
        }

        if (i + 1 == pinyin_len) {
            /* Final composition: show marked text and first candidate page. */
            fputs("marked text: ", stdout);
            print_slice(result.marked_text, result.marked_text_len);
            printf("\nmarked cursor (utf-8 bytes): %zu\n", result.marked_cursor_utf8);
            printf("candidates (page %zu, %zu shown):\n",
                   result.page_no, result.candidate_count);
            for (size_t c = 0; c < result.candidate_count; ++c) {
                const AimCandidate *cand = &result.candidates[c];
                printf("  %zu. ", c + 1);
                print_slice(cand->text, cand->text_len);
                fputs("\n      reading: ", stdout);
                print_slice(cand->reading_key, cand->reading_key_len);
                fputs("\n      comment: ", stdout);
                print_slice(cand->comment, cand->comment_len);
                putchar('\n');
            }
        }

        aim_result_free(&result);
    }

    /* Commit with Space. */
    memset(&event, 0, sizeof(event));
    event.kind = AIM_KEY_SPACE;
    event.text = NULL;
    event.text_len = 0;
    event.modifiers = 0;
    event.security_state = AIM_SECURITY_NON_SECURE;
    event.delivery_id = ++delivery_id;

    memset(&result, 0, sizeof(result));
    ret = aim_session_process(session, &event, &result);
    if (ret != 0 || result.status != 0) {
        print_call_error("space key", ret, &result);
        aim_result_free(&result);
        exit_code = 1;
        goto destroy;
    }

    if (result.commit_text == NULL || result.commit_text_len == 0) {
        fprintf(stderr, "zhixu-demo: final commit text is empty\n");
        aim_result_free(&result);
        exit_code = 1;
        goto destroy;
    }

    fputs("committed text: ", stdout);
    print_slice(result.commit_text, result.commit_text_len);
    putchar('\n');
    aim_result_free(&result);

destroy:
    aim_session_destroy(session);
    fflush(stdout);
    return exit_code;
}
