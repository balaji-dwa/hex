#import <CoreFoundation/CoreFoundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <ImageIO/ImageIO.h>
#include <stdlib.h>
#include <string.h>

// JPEG has no alpha: a window capture's transparent shadow and corners would
// encode black. Draw anything with alpha over white first.
static CGImageRef flatten(CGImageRef image) {
    CGImageAlphaInfo alpha = CGImageGetAlphaInfo(image);
    if (alpha == kCGImageAlphaNone || alpha == kCGImageAlphaNoneSkipLast ||
        alpha == kCGImageAlphaNoneSkipFirst) {
        return CGImageRetain(image);
    }
    size_t width = CGImageGetWidth(image);
    size_t height = CGImageGetHeight(image);
    CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    if (space == NULL) {
        return NULL;
    }
    CGContextRef context = CGBitmapContextCreate(NULL, width, height, 8, 0, space,
                                                 (CGBitmapInfo)kCGImageAlphaNoneSkipLast);
    CGColorSpaceRelease(space);
    if (context == NULL) {
        return NULL;
    }
    CGRect bounds = CGRectMake(0, 0, (CGFloat)width, (CGFloat)height);
    CGContextSetRGBFillColor(context, 1, 1, 1, 1);
    CGContextFillRect(context, bounds);
    CGContextSetInterpolationQuality(context, kCGInterpolationHigh);
    CGContextDrawImage(context, bounds, image);
    CGImageRef flat = CGBitmapContextCreateImage(context);
    CGContextRelease(context);
    return flat;
}

// Slippy's image work (`src/slippy/image.rs`): one ImageIO pass that decodes
// any image it reads (PNG, JPEG, HEIC, WebP…), scales it so its long edge is
// at most `max_edge` pixels with its orientation applied, and encodes it as a
// JPEG at `quality` (0…1), over white where it was transparent. On success
// `*out_buf` is a malloc'ed buffer the caller frees with
// `hex_slippy_free_bytes`; on failure it is NULL and 0 is returned.
int hex_slippy_encode_jpeg(const uint8_t *input, size_t input_len, uint32_t max_edge,
                           float quality, uint8_t **out_buf, size_t *out_len) {
    if (out_buf == NULL || out_len == NULL) {
        return 0;
    }
    *out_buf = NULL;
    *out_len = 0;
    if (input == NULL || input_len == 0 || max_edge == 0) {
        return 0;
    }
    @autoreleasepool {
        NSData *data = [NSData dataWithBytesNoCopy:(void *)input
                                            length:input_len
                                      freeWhenDone:NO];
        CGImageSourceRef source = CGImageSourceCreateWithData((__bridge CFDataRef)data, NULL);
        if (source == NULL) {
            return 0;
        }
        NSDictionary *thumbnail = @{
            (__bridge NSString *)kCGImageSourceCreateThumbnailFromImageAlways : @YES,
            (__bridge NSString *)kCGImageSourceCreateThumbnailWithTransform : @YES,
            (__bridge NSString *)kCGImageSourceShouldCacheImmediately : @YES,
            (__bridge NSString *)kCGImageSourceThumbnailMaxPixelSize : @(max_edge),
        };
        CGImageRef image =
            CGImageSourceCreateThumbnailAtIndex(source, 0, (__bridge CFDictionaryRef)thumbnail);
        CFRelease(source);
        if (image == NULL) {
            return 0;
        }
        CGImageRef opaque = flatten(image);
        CGImageRelease(image);
        if (opaque == NULL) {
            return 0;
        }
        image = opaque;
        NSMutableData *output = [NSMutableData data];
        CGImageDestinationRef destination = CGImageDestinationCreateWithData(
            (__bridge CFMutableDataRef)output, CFSTR("public.jpeg"), 1, NULL);
        if (destination == NULL) {
            CGImageRelease(image);
            return 0;
        }
        NSDictionary *options = @{
            (__bridge NSString *)kCGImageDestinationLossyCompressionQuality : @(quality),
        };
        CGImageDestinationAddImage(destination, image, (__bridge CFDictionaryRef)options);
        bool finished = CGImageDestinationFinalize(destination);
        CFRelease(destination);
        CGImageRelease(image);
        if (!finished || output.length == 0) {
            return 0;
        }
        uint8_t *buffer = malloc(output.length);
        if (buffer == NULL) {
            return 0;
        }
        memcpy(buffer, output.bytes, output.length);
        *out_buf = buffer;
        *out_len = output.length;
        return 1;
    }
}

void hex_slippy_free_bytes(uint8_t *buffer) { free(buffer); }

// The pixel size of the image `input` holds, read from its header alone:
// 1 with `*width` and `*height` set, or 0 when ImageIO can't read it.
int hex_slippy_image_size(const uint8_t *input, size_t input_len, uint32_t *width,
                          uint32_t *height) {
    if (input == NULL || input_len == 0 || width == NULL || height == NULL) {
        return 0;
    }
    @autoreleasepool {
        NSData *data = [NSData dataWithBytesNoCopy:(void *)input
                                            length:input_len
                                      freeWhenDone:NO];
        CGImageSourceRef source = CGImageSourceCreateWithData((__bridge CFDataRef)data, NULL);
        if (source == NULL) {
            return 0;
        }
        NSDictionary *properties =
            CFBridgingRelease(CGImageSourceCopyPropertiesAtIndex(source, 0, NULL));
        CFRelease(source);
        NSNumber *w = properties[(__bridge NSString *)kCGImagePropertyPixelWidth];
        NSNumber *h = properties[(__bridge NSString *)kCGImagePropertyPixelHeight];
        if (w == nil || h == nil || w.unsignedIntValue == 0 || h.unsignedIntValue == 0) {
            return 0;
        }
        NSNumber *orientation = properties[(__bridge NSString *)kCGImagePropertyOrientation];
        // Orientations 5–8 turn the image a quarter.
        bool turned = orientation != nil && orientation.intValue >= 5;
        *width = turned ? h.unsignedIntValue : w.unsignedIntValue;
        *height = turned ? w.unsignedIntValue : h.unsignedIntValue;
        return 1;
    }
}
