SOURCE	 = PROTOCOL.in.md
OUT	     = PROTOCOL.md
MMDC	 = npx -y -p @mermaid-js/mermaid-cli@latest mmdc
FLAGS	 = -t dark -b transparent
IMAGES   = $(wildcard *.svg)

all: $(OUT)

PROTOCOL.md: PROTOCOL.in.md
	$(MMDC) $(FLAGS) -i PROTOCOL.in.md -o PROTOCOL.md

clean:
	rm -f $(OUT) $(IMAGES)
