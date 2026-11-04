// Offline SoundAnalysis capability/cost probe. No capture device or app pipeline.
// Build: xcrun clang -fobjc-arc -Wall -Wextra -Werror -framework Foundation
//        -framework AVFoundation -framework SoundAnalysis -framework CoreMedia scripts/sound-analysis-spike.m -o /tmp/sound-analysis-spike
#import <Foundation/Foundation.h>
#import <AVFoundation/AVFoundation.h>
#import <SoundAnalysis/SoundAnalysis.h>
#include <sys/resource.h>
#include <time.h>

static double now(void) {
    struct timespec value;
    clock_gettime(CLOCK_MONOTONIC, &value);
    return value.tv_sec + value.tv_nsec / 1e9;
}
static double cpu(struct rusage value) {
    return value.ru_utime.tv_sec + value.ru_stime.tv_sec
        + (value.ru_utime.tv_usec + value.ru_stime.tv_usec) / 1e6;
}

@interface ProbeObserver : NSObject <SNResultsObserving>
@property NSLock *lock;
@property double started;
@property NSNumber *firstMilliseconds;
@property NSMutableArray *windows;
@property NSString *failure;
@property BOOL completed;
@end
@implementation ProbeObserver
- (instancetype)init {
    self = [super init];
    if (self) { _lock = [NSLock new]; _windows = [NSMutableArray new]; }
    return self;
}
- (void)request:(id<SNRequest>)request didProduceResult:(id<SNResult>)result {
    (void)request;
    if (![result isKindOfClass:[SNClassificationResult class]]) return;
    SNClassificationResult *classification = (SNClassificationResult *)result;
    [self.lock lock];
    if (!self.firstMilliseconds) self.firstMilliseconds = @((now() - self.started) * 1000);
    NSMutableArray *top = [NSMutableArray new];
    for (SNClassification *item in classification.classifications) {
        [top addObject:@{@"label": item.identifier, @"confidence": @(item.confidence)}];
        if (top.count == 3) break;
    }
    [self.windows addObject:@{@"start_s": @(CMTimeGetSeconds(classification.timeRange.start)),
        @"duration_s": @(CMTimeGetSeconds(classification.timeRange.duration)), @"top": top}];
    [self.lock unlock];
}
- (void)request:(id<SNRequest>)request didFailWithError:(NSError *)error {
    (void)request;
    [self.lock lock]; self.failure = error.localizedDescription; [self.lock unlock];
}
- (void)requestDidComplete:(id<SNRequest>)request {
    (void)request;
    [self.lock lock]; self.completed = YES; [self.lock unlock];
}
@end

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc != 2) { fprintf(stderr, "usage: sound-analysis-spike SHORT_FIXTURE.wav\n"); return 2; }
        NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:argv[1]]];
        NSError *error = nil;
        AVAudioFile *audio = [[AVAudioFile alloc] initForReading:url error:&error];
        if (!audio || audio.length / audio.processingFormat.sampleRate > 15) {
            fprintf(stderr, "fixture must be readable and at most 15 seconds\n"); return 2;
        }
        struct rusage before, after;
        getrusage(RUSAGE_SELF, &before);
        double start = now();
        SNClassifySoundRequest *request = [[SNClassifySoundRequest alloc]
            initWithClassifierIdentifier:SNClassifierIdentifierVersion1 error:&error];
        if (!request) { fprintf(stderr, "classifier unavailable: %s\n", error.localizedDescription.UTF8String); return 1; }
        double initialized = now();
        SNAudioFileAnalyzer *analyzer = [[SNAudioFileAnalyzer alloc] initWithURL:url error:&error];
        ProbeObserver *observer = [ProbeObserver new];
        if (!analyzer || ![analyzer addRequest:request withObserver:observer error:&error]) {
            fprintf(stderr, "analyzer unavailable: %s\n", error.localizedDescription.UTF8String); return 1;
        }
        observer.started = now();
        [analyzer analyze];
        double finish = now();
        getrusage(RUSAGE_SELF, &after);
        [observer.lock lock];
        NSDictionary *report = @{
            @"schema": @"codescribe.sound-analysis-spike.v1",
            @"os": NSProcessInfo.processInfo.operatingSystemVersionString,
            @"fixture": url.lastPathComponent,
            @"fixture_seconds": @(audio.length / audio.processingFormat.sampleRate),
            @"classifier": @"version1", @"known_classes": request.knownClassifications,
            @"window_seconds": @(CMTimeGetSeconds(request.windowDuration)),
            @"overlap_factor": @(request.overlapFactor),
            @"initialization_ms": @((initialized - start) * 1000),
            @"analysis_ms": @((finish - observer.started) * 1000),
            @"first_result_ms": observer.firstMilliseconds ?: [NSNull null],
            @"total_ms": @((finish - start) * 1000),
            @"cpu_seconds": @(cpu(after) - cpu(before)),
            @"cpu_average_cores": @((cpu(after) - cpu(before)) / (finish - start)),
            @"peak_rss_bytes": @(after.ru_maxrss),
            @"thermal_state": @(NSProcessInfo.processInfo.thermalState),
            @"completed": @(observer.completed),
            @"failure": observer.failure ?: [NSNull null], @"windows": observer.windows,
            @"accelerator_cost": [NSNull null]
        };
        NSData *json = [NSJSONSerialization dataWithJSONObject:report options:NSJSONWritingPrettyPrinted | NSJSONWritingSortedKeys error:&error];
        if (!json) { [observer.lock unlock]; return 1; }
        fwrite(json.bytes, 1, json.length, stdout); fputc('\n', stdout);
        BOOL okay = observer.completed && !observer.failure;
        [observer.lock unlock];
        return okay ? 0 : 1;
    }
}
