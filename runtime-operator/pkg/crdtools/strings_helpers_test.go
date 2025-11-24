package crdtools

import "testing"

func TestCoalesce(t *testing.T) {
	tests := []struct {
		name string
		args []string
		want string
	}{
		{
			name: "empty",
			args: []string{""},
			want: "",
		},
		{
			name: "first",
			args: []string{"first", "second"},
			want: "first",
		},
		{
			name: "second",
			args: []string{"", "second"},
			want: "second",
		},
		{
			name: "third",
			args: []string{"", "", "third"},
			want: "third",
		},
		{
			name: "fourth",
			args: []string{"", "", "", "fourth"},
			want: "fourth",
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := Coalesce(tt.args...); got != tt.want {
				t.Errorf("Coalesce() = %v, want %v", got, tt.want)
			}
		})
	}
}
