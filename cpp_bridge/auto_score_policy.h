#pragma once

#include <cmath>
#include <cstdint>
#include <utility>
#include <vector>

struct AutoScorePolicy {
    // music_metas auto fields were generated with the ordinary 0.7 judgment.
    static constexpr double baseline = 0.7;
    double coefficient = baseline;
    std::vector<std::pair<std::int64_t, std::int64_t>> finale_windows;

    double active_coefficient(bool is_auto, std::int64_t now) const {
        if (!is_auto || !std::isfinite(coefficient) || coefficient <= baseline + 0.000001) {
            return baseline;
        }
        for (const auto& [start, end] : finale_windows) {
            if (start > 0 && end > start && now >= start && now < end) return coefficient;
        }
        return baseline;
    }
};
