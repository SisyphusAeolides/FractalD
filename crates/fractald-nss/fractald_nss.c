#define _GNU_SOURCE

#include <arpa/inet.h>
#include <arpa/nameser.h>
#include <errno.h>
#include <netdb.h>
#include <nss.h>
#include <resolv.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/socket.h>

#define FRACTALD_NSS_MAX_ADDRESSES 32U
#define FRACTALD_NSS_PACKET_SIZE 4096U

struct fractald_nss_address {
    int family;
    unsigned char bytes[16];
};

static enum nss_status fractald_nss_query_error(int *errnop, int *h_errnop)
{
    if (h_errno == TRY_AGAIN) {
        *errnop = EAGAIN;
        *h_errnop = TRY_AGAIN;
        return NSS_STATUS_TRYAGAIN;
    }
    if (h_errno == HOST_NOT_FOUND || h_errno == NO_DATA) {
        *errnop = ENOENT;
        *h_errnop = h_errno;
        return NSS_STATUS_NOTFOUND;
    }
    *errnop = EIO;
    *h_errnop = NO_RECOVERY;
    return NSS_STATUS_UNAVAIL;
}

static int fractald_nss_query(
    const char *name,
    int type,
    struct fractald_nss_address *addresses,
    size_t capacity,
    size_t *count,
    uint32_t *minimum_ttl)
{
    unsigned char packet[FRACTALD_NSS_PACKET_SIZE];
    int length = res_query(name, C_IN, type, packet, sizeof(packet));
    if (length < 0) {
        return -1;
    }

    ns_msg message;
    if (ns_initparse(packet, length, &message) < 0) {
        h_errno = NO_RECOVERY;
        return -1;
    }

    size_t found = 0;
    uint32_t ttl = UINT32_MAX;
    int answers = ns_msg_count(message, ns_s_an);
    for (int index = 0; index < answers; ++index) {
        ns_rr record;
        if (ns_parserr(&message, ns_s_an, index, &record) < 0) {
            h_errno = NO_RECOVERY;
            return -1;
        }
        if ((int)ns_rr_type(record) != type) {
            continue;
        }
        size_t length_for_type = type == ns_t_a ? 4U : 16U;
        if (ns_rr_rdlen(record) != length_for_type) {
            continue;
        }
        if (found >= capacity) {
            break;
        }
        addresses[found].family = type == ns_t_a ? AF_INET : AF_INET6;
        memcpy(addresses[found].bytes, ns_rr_rdata(record), length_for_type);
        ++found;
        if (ns_rr_ttl(record) < ttl) {
            ttl = ns_rr_ttl(record);
        }
    }

    *count = found;
    *minimum_ttl = found == 0 ? 0 : ttl;
    return 0;
}

static int fractald_nss_reverse_query(
    const void *address,
    socklen_t address_length,
    int family,
    char *name,
    size_t name_length,
    uint32_t *ttl)
{
    const unsigned char *bytes = address;
    char query[80];
    size_t offset = 0;
    if (family == AF_INET && address_length == 4U) {
        int written = snprintf(
            query,
            sizeof(query),
            "%u.%u.%u.%u.in-addr.arpa",
            bytes[3],
            bytes[2],
            bytes[1],
            bytes[0]);
        if (written < 0 || (size_t)written >= sizeof(query)) {
            h_errno = NO_RECOVERY;
            return -1;
        }
    } else if (family == AF_INET6 && address_length == 16U) {
        static const char hex[] = "0123456789abcdef";
        for (int index = 15; index >= 0; --index) {
            if (offset + 4U >= sizeof(query)) {
                h_errno = NO_RECOVERY;
                return -1;
            }
            query[offset++] = hex[bytes[index] & 0x0fU];
            query[offset++] = '.';
            query[offset++] = hex[(bytes[index] >> 4U) & 0x0fU];
            query[offset++] = '.';
        }
        memcpy(query + offset, "ip6.arpa", sizeof("ip6.arpa"));
    } else {
        h_errno = NO_DATA;
        return 0;
    }

    unsigned char packet[FRACTALD_NSS_PACKET_SIZE];
    int length = res_query(query, C_IN, ns_t_ptr, packet, sizeof(packet));
    if (length < 0) {
        return -1;
    }
    ns_msg message;
    if (ns_initparse(packet, length, &message) < 0) {
        h_errno = NO_RECOVERY;
        return -1;
    }
    int answers = ns_msg_count(message, ns_s_an);
    for (int index = 0; index < answers; ++index) {
        ns_rr record;
        if (ns_parserr(&message, ns_s_an, index, &record) < 0) {
            h_errno = NO_RECOVERY;
            return -1;
        }
        if ((int)ns_rr_type(record) != ns_t_ptr) {
            continue;
        }
        int expanded = dn_expand(
            packet,
            packet + length,
            ns_rr_rdata(record),
            name,
            (int)name_length);
        if (expanded < 0) {
            h_errno = NO_RECOVERY;
            return -1;
        }
        *ttl = ns_rr_ttl(record);
        return 1;
    }
    h_errno = NO_DATA;
    return 0;
}

static int fractald_nss_align(
    char *buffer,
    size_t buflen,
    size_t *offset,
    size_t alignment,
    size_t size,
    void **result)
{
    uintptr_t address = (uintptr_t)buffer + *offset;
    uintptr_t aligned = (address + alignment - 1U) & ~(alignment - 1U);
    size_t padding = (size_t)(aligned - address);
    if (padding > buflen - *offset || size > buflen - *offset - padding) {
        return -1;
    }
    *offset += padding;
    *result = buffer + *offset;
    *offset += size;
    return 0;
}

static enum nss_status fractald_nss_hostent(
    const char *name,
    int family,
    const struct fractald_nss_address *addresses,
    size_t count,
    struct hostent *result,
    char *buffer,
    size_t buflen,
    int *errnop,
    int *h_errnop)
{
    size_t offset = 0;
    char **address_list;
    char **aliases;
    char *canonical;
    unsigned char *address_data;
    size_t address_length = family == AF_INET ? 4U : 16U;
    size_t pointer_count = count + 1U;
    size_t name_length = strlen(name) + 1U;

    if (fractald_nss_align(
            buffer,
            buflen,
            &offset,
            _Alignof(char *),
            pointer_count * sizeof(*address_list),
            (void **)&address_list) < 0
        || fractald_nss_align(
            buffer,
            buflen,
            &offset,
            _Alignof(char *),
            sizeof(*aliases),
            (void **)&aliases) < 0
        || fractald_nss_align(
            buffer,
            buflen,
            &offset,
            _Alignof(char),
            name_length,
            (void **)&canonical) < 0
        || fractald_nss_align(
            buffer,
            buflen,
            &offset,
            _Alignof(char),
            count * address_length,
            (void **)&address_data) < 0) {
        *errnop = ERANGE;
        *h_errnop = NETDB_INTERNAL;
        return NSS_STATUS_TRYAGAIN;
    }

    memcpy(canonical, name, name_length);
    aliases[0] = NULL;
    for (size_t index = 0; index < count; ++index) {
        address_list[index] = (char *)address_data + index * address_length;
        memcpy(address_list[index], addresses[index].bytes, address_length);
    }
    address_list[count] = NULL;

    result->h_name = canonical;
    result->h_aliases = aliases;
    result->h_addrtype = family;
    result->h_length = (int)address_length;
    result->h_addr_list = address_list;
    *errnop = 0;
    *h_errnop = NETDB_SUCCESS;
    return NSS_STATUS_SUCCESS;
}

enum nss_status _nss_fractald_gethostbyname2_r(
    const char *name,
    int family,
    struct hostent *result,
    char *buffer,
    size_t buflen,
    int *errnop,
    int *h_errnop)
{
    if (family != AF_INET && family != AF_INET6) {
        *errnop = EAFNOSUPPORT;
        *h_errnop = NO_DATA;
        return NSS_STATUS_NOTFOUND;
    }

    struct fractald_nss_address addresses[FRACTALD_NSS_MAX_ADDRESSES];
    size_t count = 0;
    uint32_t ttl = 0;
    int type = family == AF_INET ? ns_t_a : ns_t_aaaa;
    if (fractald_nss_query(name, type, addresses, FRACTALD_NSS_MAX_ADDRESSES, &count, &ttl) < 0) {
        return fractald_nss_query_error(errnop, h_errnop);
    }
    (void)ttl;
    if (count == 0) {
        *errnop = ENOENT;
        *h_errnop = NO_DATA;
        return NSS_STATUS_NOTFOUND;
    }
    return fractald_nss_hostent(
        name,
        family,
        addresses,
        count,
        result,
        buffer,
        buflen,
        errnop,
        h_errnop);
}

enum nss_status _nss_fractald_gethostbyname3_r(
    const char *name,
    int family,
    struct hostent *result,
    char *buffer,
    size_t buflen,
    int *errnop,
    int *h_errnop,
    int32_t *ttlp,
    char **canonp)
{
    enum nss_status status = _nss_fractald_gethostbyname2_r(
        name, family, result, buffer, buflen, errnop, h_errnop);
    if (status == NSS_STATUS_SUCCESS) {
        if (ttlp != NULL) {
            *ttlp = 0;
        }
        if (canonp != NULL) {
            *canonp = result->h_name;
        }
    }
    return status;
}

enum nss_status _nss_fractald_gethostbyname_r(
    const char *name,
    struct hostent *result,
    char *buffer,
    size_t buflen,
    int *errnop,
    int *h_errnop)
{
    return _nss_fractald_gethostbyname2_r(
        name, AF_INET, result, buffer, buflen, errnop, h_errnop);
}

enum nss_status _nss_fractald_gethostbyname4_r(
    const char *name,
    struct gaih_addrtuple **pat,
    char *buffer,
    size_t buflen,
    int *errnop,
    int *h_errnop,
    int32_t *ttlp)
{
    struct fractald_nss_address addresses[FRACTALD_NSS_MAX_ADDRESSES];
    size_t count = 0;
    uint32_t minimum_ttl = UINT32_MAX;
    uint32_t ttl;
    size_t found;
    int query_error = 0;

    if (fractald_nss_query(
            name,
            ns_t_a,
            addresses,
            FRACTALD_NSS_MAX_ADDRESSES,
            &found,
            &ttl) < 0) {
        query_error = h_errno;
    } else {
        count = found;
        if (found > 0) {
            minimum_ttl = ttl;
        }
    }

    if (count < FRACTALD_NSS_MAX_ADDRESSES
        && fractald_nss_query(
               name,
               ns_t_aaaa,
               addresses + count,
               FRACTALD_NSS_MAX_ADDRESSES - count,
               &found,
               &ttl) < 0) {
        if (query_error == 0) {
            query_error = h_errno;
        }
    } else {
        count += found;
        if (found > 0 && ttl < minimum_ttl) {
            minimum_ttl = ttl;
        }
    }

    if (count == 0) {
        if (query_error == TRY_AGAIN) {
            *errnop = EAGAIN;
            *h_errnop = TRY_AGAIN;
            return NSS_STATUS_TRYAGAIN;
        }
        *errnop = ENOENT;
        *h_errnop = query_error == 0 ? NO_DATA : query_error;
        return query_error == NO_RECOVERY ? NSS_STATUS_UNAVAIL : NSS_STATUS_NOTFOUND;
    }

    size_t offset = 0;
    struct gaih_addrtuple *tuples;
    char *canonical;
    size_t name_length = strlen(name) + 1U;
    if (fractald_nss_align(
            buffer,
            buflen,
            &offset,
            _Alignof(struct gaih_addrtuple),
            count * sizeof(*tuples),
            (void **)&tuples) < 0
        || fractald_nss_align(
            buffer,
            buflen,
            &offset,
            _Alignof(char),
            name_length,
            (void **)&canonical) < 0) {
        *errnop = ERANGE;
        *h_errnop = NETDB_INTERNAL;
        return NSS_STATUS_TRYAGAIN;
    }
    memcpy(canonical, name, name_length);

    for (size_t index = 0; index < count; ++index) {
        memset(&tuples[index], 0, sizeof(tuples[index]));
        tuples[index].name = canonical;
        tuples[index].family = addresses[index].family;
        memcpy(
            tuples[index].addr,
            addresses[index].bytes,
            addresses[index].family == AF_INET ? 4U : 16U);
        tuples[index].next = index + 1U < count ? &tuples[index + 1U] : NULL;
    }
    *pat = tuples;
    *errnop = 0;
    *h_errnop = NETDB_SUCCESS;
    if (ttlp != NULL) {
        *ttlp = minimum_ttl == UINT32_MAX ? 0 : (int32_t)minimum_ttl;
    }
    return NSS_STATUS_SUCCESS;
}

enum nss_status _nss_fractald_gethostbyaddr2_r(
    const void *address,
    socklen_t address_length,
    int family,
    struct hostent *result,
    char *buffer,
    size_t buflen,
    int *errnop,
    int *h_errnop,
    int32_t *ttlp)
{
    if ((family != AF_INET && family != AF_INET6)
        || address == NULL
        || address_length != (family == AF_INET ? 4U : 16U)) {
        *errnop = EAFNOSUPPORT;
        *h_errnop = NO_DATA;
        return NSS_STATUS_NOTFOUND;
    }

    char reverse_name[NI_MAXHOST];
    uint32_t ttl = 0;
    int status = fractald_nss_reverse_query(
        address,
        address_length,
        family,
        reverse_name,
        sizeof(reverse_name),
        &ttl);
    if (status < 0) {
        return fractald_nss_query_error(errnop, h_errnop);
    }
    if (status == 0) {
        *errnop = ENOENT;
        *h_errnop = NO_DATA;
        return NSS_STATUS_NOTFOUND;
    }

    struct fractald_nss_address item;
    memset(&item, 0, sizeof(item));
    item.family = family;
    memcpy(item.bytes, address, address_length);
    enum nss_status result_status = fractald_nss_hostent(
        reverse_name,
        family,
        &item,
        1,
        result,
        buffer,
        buflen,
        errnop,
        h_errnop);
    if (result_status == NSS_STATUS_SUCCESS && ttlp != NULL) {
        *ttlp = (int32_t)ttl;
    }
    return result_status;
}

enum nss_status _nss_fractald_gethostbyaddr_r(
    const void *address,
    socklen_t address_length,
    int family,
    struct hostent *result,
    char *buffer,
    size_t buflen,
    int *errnop,
    int *h_errnop)
{
    return _nss_fractald_gethostbyaddr2_r(
        address,
        address_length,
        family,
        result,
        buffer,
        buflen,
        errnop,
        h_errnop,
        NULL);
}
