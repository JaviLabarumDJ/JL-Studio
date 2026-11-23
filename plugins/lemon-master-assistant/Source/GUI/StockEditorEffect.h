// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <JuceHeader.h>
#include <cstdint>
namespace mlstock {
using Present = uint64_t (*)(uint8_t*, uint32_t, uint32_t, uint32_t, float, uint32_t);
Present presenter();
}

namespace mlstock {
// JUCE paints the complete component tree into the effect image before this callback.
// The timer only invalidates the editor when the host's appearance changes.
class StockEditorEffect final : private juce::ImageEffectFilter, private juce::Timer {
public:
    explicit StockEditorEffect(juce::Component& owner) : owner_(owner) {
        if (presenter()) { timerCallback(); startTimerHz(20); }
    }
    ~StockEditorEffect() override {
        stopTimer();
        if (key_) owner_.setComponentEffect(nullptr);
    }
private:
    juce::Component& owner_;
    uint64_t key_ = 0;
    void timerCallback() override {
        const auto next = presenter()(nullptr, 0, 0, 0, 1.0f, 0);
        if (key_ == next) return;
        const bool wasEnabled = key_ != 0;
        key_ = next;
        if (wasEnabled != (key_ != 0)) owner_.setComponentEffect(key_ ? this : nullptr);
        owner_.repaint();
    }
    void applyEffect(juce::Image& source, juce::Graphics& g, float scale, float alpha) override {
        auto image = source.convertedToFormat(juce::Image::ARGB);
        image.duplicateIfShared();
        {
            juce::Image::BitmapData pixels(image, juce::Image::BitmapData::readWrite);
            presenter()(pixels.data, uint32_t(pixels.width), uint32_t(pixels.height),
                        uint32_t(pixels.lineStride), scale, 1);
        }
        g.setOpacity(alpha);
        g.drawImageAt(image, 0, 0);
    }
};
}
