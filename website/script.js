// Octomind Website JavaScript
// Simple, clean animations and interactions

// GitHub Stars Fetching
async function fetchGitHubStars() {
	try {
		const response = await fetch('https://api.github.com/repos/muvon/octomind');
		const data = await response.json();
		const starsElement = document.getElementById('github-stars');
		const starCount = starsElement.querySelector('.star-count');

		if (data.stargazers_count !== undefined) {
			// Animate the number counting up
			animateNumber(starCount, 0, data.stargazers_count, 1000);
		} else {
			starCount.textContent = '103';
		}
	} catch (error) {
		console.log('Could not fetch GitHub stars');
		const starCount = document.getElementById('github-stars').querySelector('.star-count');
		starCount.textContent = '103';
	}
}

// Animate number counting
function animateNumber(element, start, end, duration) {
	const startTime = performance.now();

	function updateNumber(currentTime) {
		const elapsed = currentTime - startTime;
		const progress = Math.min(elapsed / duration, 1);

		// Easing function for smooth animation
		const easeOutQuart = 1 - Math.pow(1 - progress, 4);
		const current = Math.floor(start + (end - start) * easeOutQuart);

		element.textContent = current;

		if (progress < 1) {
			requestAnimationFrame(updateNumber);
		} else {
			element.textContent = end;
		}
	}

	requestAnimationFrame(updateNumber);
}

// Typing effect for hero subtitle
function createTypingEffect(element, text, speed = 50) {
	element.textContent = '';
	element.style.borderRight = '2px solid #7CB342';

	let i = 0;
	function typeChar() {
		if (i < text.length) {
			element.textContent += text.charAt(i);
			i++;
			setTimeout(typeChar, speed);
		} else {
			// Remove cursor after typing is complete
			setTimeout(() => {
				element.style.borderRight = 'none';
			}, 1000);
		}
	}

	// Start typing after a short delay
	setTimeout(typeChar, 500);
}

// Smooth scrolling for anchor links
function initSmoothScrolling() {
	document.querySelectorAll('a[href^="#"]').forEach(anchor => {
		anchor.addEventListener('click', function (e) {
			e.preventDefault();
			const target = document.querySelector(this.getAttribute('href'));
			if (target) {
				target.scrollIntoView({
					behavior: 'smooth',
					block: 'start'
				});
			}
		});
	});
}

// Intersection Observer for fade-in animations
function initScrollAnimations() {
	const observerOptions = {
		threshold: 0.1,
		rootMargin: '0px 0px -50px 0px'
	};

	const observer = new IntersectionObserver((entries) => {
		entries.forEach(entry => {
			if (entry.isIntersecting) {
				entry.target.style.opacity = '1';
				entry.target.style.transform = 'translateY(0)';
			}
		});
	}, observerOptions);

	// Observe elements that should fade in
	const animatedElements = document.querySelectorAll('.feature, .tool, .config-item, .highlight, .command-group');
	animatedElements.forEach(el => {
		el.style.opacity = '0';
		el.style.transform = 'translateY(20px)';
		el.style.transition = 'opacity 0.6s ease, transform 0.6s ease';
		observer.observe(el);
	});
}

// Add subtle hover effects to conversation examples
function initConversationEffects() {
	const examples = document.querySelectorAll('.example');
	examples.forEach((example, index) => {
		example.style.opacity = '0';
		example.style.transform = 'translateX(-20px)';
		example.style.transition = 'all 0.5s ease';

		// Stagger the animation
		setTimeout(() => {
			example.style.opacity = '1';
			example.style.transform = 'translateX(0)';
		}, index * 200);
	});
}

// Add typing effect to code examples in conversation
function initCodeTypingEffects() {
	const codeElements = document.querySelectorAll('.example .user-message');

	codeElements.forEach((element, index) => {
		const originalText = element.innerHTML;

		// Only apply typing effect to specific examples
		if (index === 0 || index === 3) { // First and agent examples
			element.innerHTML = '<span class="message-label">You:</span>';

			setTimeout(() => {
				const textContent = originalText.replace('<span class="message-label">You:</span>', '');
				let currentText = '<span class="message-label">You:</span>';
				let i = 0;

				function typeChar() {
					if (i < textContent.length) {
						currentText += textContent.charAt(i);
						element.innerHTML = currentText;
						i++;
						setTimeout(typeChar, 30);
					}
				}

				typeChar();
			}, 1000 + index * 500);
		}
	});
}

// Add subtle parallax effect to hero background
function initParallaxEffect() {
	const heroBackground = document.querySelector('.hero-background');

	window.addEventListener('scroll', () => {
		const scrolled = window.pageYOffset;
		const rate = scrolled * -0.5;
		heroBackground.style.transform = `translateY(${rate}px)`;
	});
}

// Add loading state for GitHub stars
function initGitHubStarsLoading() {
	const starCount = document.querySelector('.star-count');
	const dots = ['', '.', '..', '...'];
	let dotIndex = 0;

	const loadingInterval = setInterval(() => {
		starCount.textContent = 'Loading' + dots[dotIndex];
		dotIndex = (dotIndex + 1) % dots.length;
	}, 500);

	// Clear loading animation when stars are fetched
	const originalFetch = window.fetchGitHubStars;
	window.fetchGitHubStars = async function() {
		clearInterval(loadingInterval);
		return originalFetch();
	};
}

// Initialize everything when DOM is loaded
document.addEventListener('DOMContentLoaded', function() {
	// Core functionality
	initSmoothScrolling();
	fetchGitHubStars();

	// Animations (with delays to create a nice sequence)
	setTimeout(() => {
		const heroSubtitle = document.querySelector('.hero-subtitle');
		if (heroSubtitle) {
			createTypingEffect(heroSubtitle, 'Session-First Architecture & Intelligent MCP Automation', 60);
		}
	}, 1000);

	setTimeout(() => {
		initScrollAnimations();
	}, 1500);

	setTimeout(() => {
		initConversationEffects();
	}, 2000);

	// Optional effects (only if user hasn't disabled animations)
	if (!window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
		initParallaxEffect();

		setTimeout(() => {
			initCodeTypingEffects();
		}, 3000);
	}
});

// Export functions for potential external use
window.octomindWebsite = {
	fetchGitHubStars,
	animateNumber,
	createTypingEffect
};
