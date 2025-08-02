#include <gst/gst.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char *argv[]) {
    if (argc != 3) {
        g_printerr("Usage: %s <target timescale> <segment number>\n", argv[0]);
        return 1;
    }

    unsigned long target_timescale = strtoul(argv[1], NULL, 10);
    if (target_timescale == 0) {
        g_printerr("Invalid target timescale");
        return 1;
    }

    unsigned long segment_number = strtoul(argv[2], NULL, 10);
    if (segment_number == 0) {
        g_printerr("Invalid segment number");
        return 1;
    }

    gst_init(NULL, NULL);

    char *pipeline_desc;
    asprintf(&pipeline_desc,
             "fdsrc !"
             "decodebin !"
             "videoconvert !"
             "vp9enc row-mt=true min-quantizer=1 max-quantizer=25 !"
             "vp9parse !"
             "dashmp4mux name=muxer manual-split=true movie-timescale=%lu "
             "start-fragment-sequence-number=%lu !"
             "fdsink",
             target_timescale, segment_number);
    GstElement *pipeline = gst_parse_launch(pipeline_desc, NULL);
    g_assert(pipeline != NULL);

    GstElement *muxer = gst_bin_get_by_name(GST_BIN(pipeline), "muxer");
    g_assert(muxer != NULL);
    GstPad *muxer_pad = gst_element_get_static_pad(muxer, "sink");
    g_assert(muxer_pad != NULL);
    g_object_set(muxer_pad, "trak-timescale", target_timescale, NULL);

    gst_object_unref(muxer_pad);
    gst_object_unref(muxer);

    gst_element_set_state(pipeline, GST_STATE_PLAYING);

    GstBus *bus = gst_element_get_bus(pipeline);
    GstMessage *msg =
        gst_bus_timed_pop_filtered(bus, GST_CLOCK_TIME_NONE, GST_MESSAGE_ERROR | GST_MESSAGE_EOS);

    int ret = 0;
    if (GST_MESSAGE_TYPE(msg) == GST_MESSAGE_ERROR) {
        ret = 1;
    }

    gst_message_unref(msg);
    gst_object_unref(bus);
    gst_element_set_state(pipeline, GST_STATE_NULL);
    gst_object_unref(pipeline);

    return ret;
}
