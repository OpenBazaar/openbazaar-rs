import { useMemo, useState } from "react";
import {
  FlatList,
  StyleSheet,
  Text,
  TextInput,
  View,
} from "react-native";

import { catalog, type ListingHeader } from "./src/catalog";
import { filterListings } from "./src/search";

function formatSats(price: number): string {
  return `${price.toLocaleString("en-US")} sats`;
}

function ListingRow({ listing }: { listing: ListingHeader }) {
  return (
    <View style={styles.row}>
      <View style={styles.rowText}>
        <Text style={styles.title}>{listing.title}</Text>
        <Text style={styles.meta}>
          {listing.category} · {listing.shipFrom}
        </Text>
      </View>
      <Text style={styles.price}>{formatSats(listing.priceSats)}</Text>
    </View>
  );
}

export default function App() {
  const [query, setQuery] = useState("");
  const listings = catalog.listings;
  const visible = useMemo(
    () => filterListings(listings, query),
    [listings, query],
  );

  return (
    <View style={styles.page}>
      <View style={styles.sheet}>
        <Text style={styles.kicker}>Store</Text>
        <Text style={styles.name}>{catalog.store.name}</Text>
        <Text style={styles.region}>Ships from {catalog.store.region}</Text>
        <TextInput
          value={query}
          onChangeText={setQuery}
          placeholder="Search title or category"
          placeholderTextColor="#8a8178"
          style={styles.search}
          autoCapitalize="none"
          autoCorrect={false}
          clearButtonMode="while-editing"
          accessibilityLabel="Search listings"
        />
        {listings.length === 0 ? (
          <Text style={styles.status}>No listings are loaded.</Text>
        ) : visible.length === 0 ? (
          <Text style={styles.status}>Nothing matches “{query.trim()}”.</Text>
        ) : (
          <FlatList
            data={visible}
            keyExtractor={(item) => item.hash}
            renderItem={({ item }) => <ListingRow listing={item} />}
            ItemSeparatorComponent={() => <View style={styles.rule} />}
          />
        )}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  page: {
    flex: 1,
    backgroundColor: "#f3efe8",
    alignItems: "center",
  },
  sheet: {
    flex: 1,
    width: "100%",
    maxWidth: 640,
    paddingHorizontal: 24,
    paddingTop: 36,
    paddingBottom: 28,
  },
  kicker: {
    fontSize: 12,
    letterSpacing: 1.1,
    textTransform: "uppercase",
    color: "#6f675f",
  },
  name: {
    marginTop: 6,
    fontSize: 28,
    lineHeight: 34,
    color: "#1c1915",
    fontFamily: "Georgia",
  },
  region: {
    marginTop: 4,
    fontSize: 15,
    lineHeight: 20,
    color: "#6f675f",
  },
  search: {
    marginTop: 22,
    marginBottom: 8,
    borderWidth: 1,
    borderColor: "#ddd4c8",
    backgroundColor: "#faf7f2",
    borderRadius: 8,
    paddingHorizontal: 14,
    paddingVertical: 12,
    fontSize: 16,
    color: "#1c1915",
  },
  row: {
    flexDirection: "row",
    alignItems: "baseline",
    justifyContent: "space-between",
    gap: 16,
    paddingVertical: 14,
  },
  rowText: {
    flex: 1,
  },
  title: {
    fontSize: 17,
    lineHeight: 22,
    color: "#1c1915",
  },
  meta: {
    marginTop: 4,
    fontSize: 13,
    lineHeight: 18,
    color: "#6f675f",
  },
  price: {
    fontSize: 15,
    lineHeight: 20,
    color: "#1c1915",
  },
  rule: {
    height: 1,
    backgroundColor: "#e6dfd4",
  },
  status: {
    marginTop: 16,
    fontSize: 16,
    lineHeight: 22,
    color: "#6f675f",
  },
});
