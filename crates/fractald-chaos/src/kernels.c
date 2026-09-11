#include "fractald_chaos.h"

double fractald_logistic_step(double parameter, double value)
{
    return parameter * value * (1.0 - value);
}

uint32_t fractald_mandelbrot_escape(
    double real,
    double imaginary,
    uint32_t maximum_iterations,
    double escape_radius_squared
)
{
    double z_real = 0.0;
    double z_imaginary = 0.0;
    for (uint32_t iteration = 0; iteration < maximum_iterations; ++iteration) {
        double magnitude_squared = z_real * z_real + z_imaginary * z_imaginary;
        if (magnitude_squared > escape_radius_squared) {
            return iteration;
        }
        double next_real = z_real * z_real - z_imaginary * z_imaginary + real;
        z_imaginary = 2.0 * z_real * z_imaginary + imaginary;
        z_real = next_real;
    }
    return maximum_iterations;
}

