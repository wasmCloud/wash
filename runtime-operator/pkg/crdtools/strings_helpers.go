package crdtools

// Coalesce returns the first non-empty string from the input list.
func Coalesce(str ...string) string {
	for _, s := range str {
		if s != "" {
			return s
		}
	}
	return ""
}
