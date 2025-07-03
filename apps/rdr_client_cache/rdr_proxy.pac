function FindProxyForURL(url, host) {
    if (url.startsWith("http:") || url.startsWith("https:")) {
        return "PROXY localhost:4000";
    }
    return "DIRECT";
}
