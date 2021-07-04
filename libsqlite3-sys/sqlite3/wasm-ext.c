// import implementations from android bionic libc

#include <stdio.h>
#include <string.h>

size_t strcspn(const char* s1, const char* s2) {
    const char *p, *spanp;
    char c, sc;
    /*
     * Stop as soon as we find any character from s2.  Note that there
     * must be a NUL in s2; it suffices to stop when we find that, too.
     */
    for (p = s1;;) {
        c = *p++;
        spanp = s2;
        do {
            if ((sc = *spanp++) == c)
                return (p - 1 - s1);
        } while (sc != 0);
    }
    /* NOTREACHED */
}

int strcmp(const char *s1, const char *s2)
{
    while (*s1 == *s2++)
		if (*s1++ == 0)
			return (0);
	return (*(unsigned char *)s1 - *(unsigned char *)--s2);
}

size_t
strlen(const char *str)
{
    const char *s;
    for (s = str; *s; ++s)
        ;
    return (s - str);
}

int
strncmp(const char *s1, const char *s2, register size_t n)
{
    if (n == 0)
        return (0);
    do {
        if (*s1 != *s2++)
            return (*(unsigned char *)s1 - *(unsigned char *)--s2);
        if (*s1++ == 0)
            break;
    } while (--n != 0);
    return (0);
}

// import from glibc
char *
strrchr (const char *s, int c)
{
    const char *found, *p;
    c = (unsigned char) c;
    /* Since strchr is fast, we use it rather than the obvious loop.  */
    if (c == '\0')
        return strchr (s, '\0');
    found = NULL;
    while ((p = strchr (s, c)) != NULL)
    {
        found = p;
        s = p + 1;
    }
    return (char *) found;
}

char *
strchr (const char *s, int c)
{
    do {
        if (*s == c)
        {
            return (char*)s;
        }
    } while (*s++);
    return (0);
}

struct tm *
localtime_r (const void *timep, struct tm *tmp)
{
    // TODO: fix this tz conversion
    /* return __tz_convert (t, 1, tp); */
    return tmp;
}

#define min(a, b)	(a) < (b) ? a : b
/*
 * Qsort routine from Bentley & McIlroy's "Engineering a Sort Function".
 */
#define swapcode(TYPE, parmi, parmj, n) {       \
	long i = (n) / sizeof (TYPE);           \
	TYPE *pi = (TYPE *) (parmi);            \
	TYPE *pj = (TYPE *) (parmj);            \
	do {                                    \
            TYPE	t = *pi;                \
            *pi++ = *pj;                        \
            *pj++ = t;				\
        } while (--i > 0);                      \
    }
#define SWAPINIT(a, es) swaptype = ((char *)a - (char *)0) % sizeof(long) || \
	es % sizeof(long) ? 2 : es == sizeof(long)? 0 : 1;
static __inline void
swapfunc(char *a, char *b, int n, int swaptype)
{
    if (swaptype <= 1) 
        swapcode(long, a, b, n)
    else
        swapcode(char, a, b, n)
            }
#define swap(a, b)                              \
    if (swaptype == 0) {                        \
        long t = *(long *)(a);			\
        *(long *)(a) = *(long *)(b);		\
        *(long *)(b) = t;			\
    } else                                      \
        swapfunc(a, b, es, swaptype)
#define vecswap(a, b, n) 	if ((n) > 0) swapfunc(a, b, n, swaptype)
static __inline char *
med3(char *a, char *b, char *c, int (*cmp)(const void *, const void *))
{
    return cmp(a, b) < 0 ?
        (cmp(b, c) < 0 ? b : (cmp(a, c) < 0 ? c : a ))
        :(cmp(b, c) > 0 ? b : (cmp(a, c) < 0 ? a : c ));
}
void
qsort(void *aa, size_t n, size_t es, int (*cmp)(const void *, const void *))
{
    char *pa, *pb, *pc, *pd, *pl, *pm, *pn;
    int d, r, swaptype, swap_cnt;
    char *a = aa;
loop:	SWAPINIT(a, es);
    swap_cnt = 0;
    if (n < 7) {
        for (pm = (char *)a + es; pm < (char *) a + n * es; pm += es)
            for (pl = pm; pl > (char *) a && cmp(pl - es, pl) > 0;
                 pl -= es)
                swap(pl, pl - es);
        return;
    }
    pm = (char *)a + (n / 2) * es;
    if (n > 7) {
        pl = (char *)a;
        pn = (char *)a + (n - 1) * es;
        if (n > 40) {
            d = (n / 8) * es;
            pl = med3(pl, pl + d, pl + 2 * d, cmp);
            pm = med3(pm - d, pm, pm + d, cmp);
            pn = med3(pn - 2 * d, pn - d, pn, cmp);
        }
        pm = med3(pl, pm, pn, cmp);
    }
    swap(a, pm);
    pa = pb = (char *)a + es;
    
    pc = pd = (char *)a + (n - 1) * es;
    for (;;) {
        while (pb <= pc && (r = cmp(pb, a)) <= 0) {
            if (r == 0) {
                swap_cnt = 1;
                swap(pa, pb);
                pa += es;
            }
            pb += es;
        }
        while (pb <= pc && (r = cmp(pc, a)) >= 0) {
            if (r == 0) {
                swap_cnt = 1;
                swap(pc, pd);
                pd -= es;
            }
            pc -= es;
        }
        if (pb > pc)
            break;
        swap(pb, pc);
        swap_cnt = 1;
        pb += es;
        pc -= es;
    }
    if (swap_cnt == 0) {  /* Switch to insertion sort */
        for (pm = (char *) a + es; pm < (char *) a + n * es; pm += es)
            for (pl = pm; pl > (char *) a && cmp(pl - es, pl) > 0; 
                 pl -= es)
                swap(pl, pl - es);
        return;
    }
    pn = (char *)a + n * es;
    r = min(pa - (char *)a, pb - pa);
    vecswap(a, pb - r, r);
    r = min(pd - pc, pn - pd - (int)es);
    vecswap(pb, pn - r, r);
    if ((r = pb - pa) > (int)es)
        qsort(a, r / es, es, cmp);
    if ((r = pd - pc) > (int)es) { 
        /* Iterate rather than recurse to save stack space */
        a = pn - r;
        n = r / es;
        goto loop;
    }
}
