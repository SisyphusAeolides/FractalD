#ifndef FRACTALD_CHAOS_H
#define FRACTALD_CHAOS_H

#include <stdint.h>

double fractald_logistic_step(double parameter, double value);
uint32_t fractald_mandelbrot_escape(
    double real,
    double imaginary,
    uint32_t maximum_iterations,
    double escape_radius_squared
);

#endif

